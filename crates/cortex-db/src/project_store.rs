//! # cortex-db — LanceDB known_projects store
//!
//! Persistent map of known projects backed by the shared LanceDB store.
//!
//! The public API is intentionally synchronous (using the `block()` helper)
//! so synchronous callers in `cortex-mcp` and related tooling need
//! no changes to work with the new backend.

use anyhow::Result;
use arrow_array::{Array, BooleanArray, RecordBatch, RecordBatchIterator, StringArray};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use chrono::Utc;
use futures_util::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::{LanceDb, block};

// ─── Table configuration ──────────────────────────────────────────────────────

const TABLE: &str = "known_projects";

// ─── Schema ──────────────────────────────────────────────────────────────────

fn project_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("path", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("is_manual", DataType::Boolean, true),
        Field::new("last_scanned_at", DataType::Utf8, true),
    ]))
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

async fn get_table(db: &LanceDb) -> Result<lancedb::Table> {
    let schema = project_schema();
    match db.conn().open_table(TABLE).execute().await {
        Ok(t) => Ok(t),
        Err(_) => {
            let empty = RecordBatch::new_empty(schema.clone());
            let reader = RecordBatchIterator::new(vec![Ok(empty)], schema);
            Ok(db.conn().create_table(TABLE, reader).execute().await?)
        }
    }
}

fn read_str(batch: &RecordBatch, col: &str, idx: usize) -> String {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .map(|a| a.value(idx).to_string())
        .unwrap_or_default()
}

fn read_bool(batch: &RecordBatch, col: &str, idx: usize) -> bool {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
        .map(|a| a.value(idx))
        .unwrap_or(false)
}

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

// ─── Write ────────────────────────────────────────────────────────────────────

/// Upsert a project into `known_projects`.
///
/// `is_manual` is **sticky**: once a row is marked manual it stays manual
/// even when the same path is later re-discovered by the background scanner.
pub fn upsert(db: &LanceDb, path: &str, name: &str, is_manual: bool) -> Result<()> {
    block(async {
        let table = get_table(db).await?;

        // Read current is_manual value if the row exists (sticky behaviour).
        let batches: Vec<RecordBatch> = table
            .query()
            .only_if(format!("path = '{}'", esc(path)))
            .limit(1)
            .execute()
            .await?
            .try_collect()
            .await?;

        let existing_manual: bool = batches
            .iter()
            .find(|b| b.num_rows() > 0)
            .map(|b| read_bool(b, "is_manual", 0))
            .unwrap_or(false);

        let final_manual = is_manual || existing_manual;

        // Delete existing row then re-insert (LanceDB has no UPDATE).
        table.delete(&format!("path = '{}'", esc(path))).await?;

        let now = Utc::now().to_rfc3339();
        let schema = project_schema();
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![path])) as Arc<dyn Array>,
                Arc::new(StringArray::from(vec![name])) as Arc<dyn Array>,
                Arc::new(BooleanArray::from(vec![final_manual])) as Arc<dyn Array>,
                Arc::new(StringArray::from(vec![now.as_str()])) as Arc<dyn Array>,
            ],
        )?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(reader).execute().await?;
        Ok(())
    })
}

/// Delete a project by its absolute path.
pub fn delete(db: &LanceDb, path: &str) -> Result<()> {
    block(async {
        let table = get_table(db).await?;
        table.delete(&format!("path = '{}'", esc(path))).await?;
        Ok(())
    })
}

// ─── Read ─────────────────────────────────────────────────────────────────────

/// Return all known projects as a structured JSON Value.
///
/// Shape:
/// ```json
/// { "count": 3, "projects": [ { "path": "...", "name": "...", "is_manual": false, "last_scanned_at": "..." }, … ] }
/// ```
pub fn list_all(db: &LanceDb) -> Result<Value> {
    block(async {
        let table = get_table(db).await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .execute()
            .await?
            .try_collect()
            .await?;

        let mut projects: Vec<Value> = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                projects.push(json!({
                    "path":            read_str(batch, "path", i),
                    "name":            read_str(batch, "name", i),
                    "is_manual":       read_bool(batch, "is_manual", i),
                    "last_scanned_at": read_str(batch, "last_scanned_at", i),
                }));
            }
        }
        projects.sort_by(|a, b| {
            a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
        });
        Ok(json!({ "count": projects.len(), "projects": projects }))
    })
}

/// Return all project paths (used by `cortex_mesh_manage_map` action=refresh).
pub fn list_paths(db: &LanceDb) -> Result<Vec<String>> {
    block(async {
        let table = get_table(db).await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .execute()
            .await?
            .try_collect()
            .await?;

        let mut out = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                let p = read_str(batch, "path", i);
                if !p.is_empty() {
                    out.push(p);
                }
            }
        }
        Ok(out)
    })
}
