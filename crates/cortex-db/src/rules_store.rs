//! # cortex-db — LanceDB Agent Rules Store
//!
//! Persists learnt agent rules in a `agent_rules` LanceDB table so they
//! survive process restarts and can be injected at session start via the
//! `cortex://rules/active` MCP resource.
//!
//! ## Schema (`agent_rules`)
//!
//! | Column | Type | Notes                                        |
//! |--------|------|----------------------------------------------|
//! | id     | Utf8 | UUID v4 primary key                          |
//! | scope  | Utf8 | Constraint category (e.g. "refactor", "db")  |
//! | rule   | Utf8 | The constraint text                          |
//! | ts     | Utf8 | RFC 3339 timestamp                           |

use anyhow::Result;
use arrow_array::{Array, RecordBatch, RecordBatchIterator, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures_util::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

use crate::{LanceDb, block};

// ─── Constants ────────────────────────────────────────────────────────────────

const TABLE: &str = "agent_rules";

// ─── Public types ─────────────────────────────────────────────────────────────

/// A single learnt rule entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentRule {
    pub id: String,
    pub scope: String,
    pub rule: String,
    pub ts: String,
}

// ─── Schema ───────────────────────────────────────────────────────────────────

fn rules_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id",    DataType::Utf8, false),
        Field::new("scope", DataType::Utf8, true),
        Field::new("rule",  DataType::Utf8, true),
        Field::new("ts",    DataType::Utf8, true),
    ]))
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

async fn get_table(db: &LanceDb) -> Result<lancedb::Table> {
    let schema = rules_schema();
    match db.conn().open_table(TABLE).execute().await {
        Ok(t) => Ok(t),
        Err(_) => {
            let empty = RecordBatch::new_empty(schema.clone());
            let reader = RecordBatchIterator::new(vec![Ok(empty)], schema);
            Ok(db.conn().create_table(TABLE, reader).execute().await?)
        }
    }
}

/// Read a `Utf8` column value from a `RecordBatch` row.
fn read_str(batch: &RecordBatch, col: &str, idx: usize) -> String {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .map(|a| a.value(idx).to_string())
        .unwrap_or_default()
}

// ─── Write ────────────────────────────────────────────────────────────────────

/// Insert a new rule into the `agent_rules` table.
///
/// A fresh UUID is generated for each call — no deduplication is performed
/// at the DB layer (the caller can delete stale rules via [`delete_rule`]).
pub fn insert_rule(db: &LanceDb, scope: &str, rule: &str) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let ts = chrono::Utc::now().to_rfc3339();
    let id_owned    = id.clone();
    let scope_owned = scope.to_string();
    let rule_owned  = rule.to_string();
    block(async move {
        let table = get_table(db).await?;
        let schema = rules_schema();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![id_owned.as_str()]))    as Arc<dyn Array>,
                Arc::new(StringArray::from(vec![scope_owned.as_str()])) as Arc<dyn Array>,
                Arc::new(StringArray::from(vec![rule_owned.as_str()]))  as Arc<dyn Array>,
                Arc::new(StringArray::from(vec![ts.as_str()]))          as Arc<dyn Array>,
            ],
        )?;

        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(reader).execute().await?;
        Ok(id_owned)
    })
}

// ─── Read ─────────────────────────────────────────────────────────────────────

/// Return all rules, sorted by `ts` ascending (oldest first).
pub fn list_all_rules(db: &LanceDb) -> Result<Vec<AgentRule>> {
    block(async {
        let table = get_table(db).await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .limit(10_000)
            .execute()
            .await?
            .try_collect()
            .await?;

        let mut rules = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                rules.push(AgentRule {
                    id:    read_str(batch, "id",    i),
                    scope: read_str(batch, "scope", i),
                    rule:  read_str(batch, "rule",  i),
                    ts:    read_str(batch, "ts",    i),
                });
            }
        }

        rules.sort_by(|a, b| a.ts.cmp(&b.ts));
        Ok(rules)
    })
}

/// Delete a single rule by `id`.
pub fn delete_rule(db: &LanceDb, id: &str) -> Result<()> {
    block(async {
        let table = get_table(db).await?;
        table
            .delete(&format!("id = '{}'", id.replace('\'', "''")))
            .await?;
        Ok(())
    })
}

/// Return rules matching any of the given scopes, sorted by `ts` ascending.
///
/// Designed for the scope-aware session injection pattern:
/// ```text
/// list_rules_for_scopes(&db, &["global", "/path/to/project"])
/// ```
pub fn list_rules_for_scopes(db: &LanceDb, scopes: &[&str]) -> Result<Vec<AgentRule>> {
    if scopes.is_empty() {
        return list_all_rules(db);
    }
    // Build SQL IN clause: scope IN ('global', '/path/to/project')
    let quoted: Vec<String> = scopes
        .iter()
        .map(|s| format!("'{}'", s.replace('\'', "''")))
        .collect();
    let predicate = format!("scope IN ({})", quoted.join(", "));

    block(async move {
        let table = get_table(db).await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .only_if(predicate)
            .limit(10_000)
            .execute()
            .await?
            .try_collect()
            .await?;

        let mut rules = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                rules.push(AgentRule {
                    id:    read_str(batch, "id",    i),
                    scope: read_str(batch, "scope", i),
                    rule:  read_str(batch, "rule",  i),
                    ts:    read_str(batch, "ts",    i),
                });
            }
        }
        rules.sort_by(|a, b| a.ts.cmp(&b.ts));
        Ok(rules)
    })
}
