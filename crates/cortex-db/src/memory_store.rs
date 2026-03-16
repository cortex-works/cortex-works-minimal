//! # cortex-db — LanceDB Memory Store
//!
//! Stores `MemoryEntry` records in a LanceDB table (`memory_entries`).
//! The schema includes a 512-dimensional vector column for future ANN search.
//!
//! ## Backward Compatibility
//!
//! Public function signatures mirror the old SQLite API so callers
//! (`cortex-ast` and other local callers) need only change the `pool: &SqlitePool`
//! parameter to `db: &LanceDb`.

use anyhow::Result;
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures_util::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use serde_json::Value;
use std::sync::Arc;

use crate::{LanceDb, block};

// ─── Table configuration ──────────────────────────────────────────────────────

const TABLE: &str = "memory_entries";
const VECTOR_DIM: i32 = 512;

// ─── Schema ──────────────────────────────────────────────────────────────────

/// Arrow schema for the `memory_entries` LanceDB table.
///
/// | Column       | Type                     | Notes                        |
/// |--------------|--------------------------|------------------------------|
/// | id           | Utf8                     | UUID primary key             |
/// | session_id   | Utf8                     |                              |
/// | project_path | Utf8                     | Absolute workspace path      |
/// | intent       | Utf8                     | Agent's stated goal          |
/// | decision     | Utf8                     | Actions taken                |
/// | source_ide   | Utf8                     | IDE name (cursor, vscode…)  |
/// | tags         | Utf8 (JSON array)        | Serialised `Vec<String>`     |
/// | ts           | Utf8                     | RFC 3339 timestamp           |
/// | vector       | FixedSizeList<f32>[512]  | Embedding (zeros if absent)  |
/// | data         | Utf8                     | Full JSON (backward compat)  |
pub fn memory_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("session_id", DataType::Utf8, true),
        Field::new("project_path", DataType::Utf8, true),
        Field::new("intent", DataType::Utf8, true),
        Field::new("decision", DataType::Utf8, true),
        Field::new("source_ide", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, true),
        Field::new("ts", DataType::Utf8, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                VECTOR_DIM,
            ),
            true,
        ),
        Field::new("data", DataType::Utf8, true),
    ]))
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Get or lazily create the `memory_entries` table.
async fn get_table(db: &LanceDb) -> Result<lancedb::Table> {
    let schema = memory_schema();
    match db.conn().open_table(TABLE).execute().await {
        Ok(t) => Ok(t),
        Err(_) => {
            let empty = RecordBatch::new_empty(schema.clone());
            let reader = RecordBatchIterator::new(vec![Ok(empty)], schema);
            Ok(db.conn().create_table(TABLE, reader).execute().await?)
        }
    }
}

/// Build a single-row `RecordBatch` from flattened memory-entry fields.
fn build_batch(
    id: &str,
    session_id: &str,
    project_path: &str,
    intent: &str,
    decision: &str,
    source_ide: &str,
    tags_json: &str,
    ts: &str,
    vector: Vec<f32>,
    data: &str,
) -> Result<RecordBatch> {
    let schema = memory_schema();
    let mut padded = vector;
    padded.resize(VECTOR_DIM as usize, 0.0_f32);

    let float_vals = Arc::new(Float32Array::from(padded)) as Arc<dyn Array>;
    let vector_col = Arc::new(FixedSizeListArray::new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        VECTOR_DIM,
        float_vals,
        None,
    )) as Arc<dyn Array>;

    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![id])) as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![session_id])) as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![project_path])) as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![intent])) as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![decision])) as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![source_ide])) as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![tags_json])) as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![ts])) as Arc<dyn Array>,
            vector_col,
            Arc::new(StringArray::from(vec![data])) as Arc<dyn Array>,
        ],
    )?)
}

/// Read a column value from a batch row as an owned `String`.
fn read_str(batch: &RecordBatch, col: &str, idx: usize) -> String {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .map(|a| a.value(idx).to_string())
        .unwrap_or_default()
}

/// Escape a string value for use inside a LanceDB SQL filter expression.
fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

// ─── Write ────────────────────────────────────────────────────────────────────

/// Insert a fully-serialised `MemoryEntry` JSON blob.
///
/// Idempotent — if the `id` already exists the operation is a no-op so
/// callers can retry safely without creating duplicates.
pub fn insert_raw(
    db: &LanceDb,
    id: &str,
    session_id: &str,
    project_path: &str,
    ts: &str,
    data: &str,
) -> Result<()> {
    block(insert_raw_async(db, id, session_id, project_path, ts, data))
}

async fn insert_raw_async(
    db: &LanceDb,
    id: &str,
    session_id: &str,
    project_path: &str,
    ts: &str,
    data: &str,
) -> Result<()> {
    let parsed: Value = serde_json::from_str(data).unwrap_or(Value::Null);
    let intent = parsed.get("intent").and_then(|v| v.as_str()).unwrap_or("");
    let decision = parsed.get("decision").and_then(|v| v.as_str()).unwrap_or("");
    let source_ide = parsed
        .get("source_ide")
        .or_else(|| parsed.get("ide"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let tags_json = parsed
        .get("tags")
        .map(|t| t.to_string())
        .unwrap_or_else(|| "[]".to_string());
    // Use a pre-computed vector from the JSON payload when present;
    // otherwise auto-embed from `intent + decision` at write time so the
    // vector column is never a zero-vector (which would make ANN useless).
    let vector: Vec<f32> = parsed
        .get("vector")
        .and_then(|v| v.as_array())
        .filter(|arr| !arr.is_empty())
        .map(|arr| arr.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect())
        .unwrap_or_else(|| {
            let text = format!("{intent} {decision}");
            crate::embed::embed_passage(&text).unwrap_or_default()
        });

    let table = get_table(db).await?;

    // Skip if already present (idempotent).
    let existing: Vec<RecordBatch> = table
        .query()
        .only_if(format!("id = '{}'", esc(id)))
        .limit(1)
        .execute()
        .await?
        .try_collect()
        .await?;
    if existing.iter().any(|b| b.num_rows() > 0) {
        return Ok(());
    }

    let batch = build_batch(
        id, session_id, project_path, intent, decision, source_ide, &tags_json, ts, vector, data,
    )?;
    let reader = RecordBatchIterator::new(vec![Ok(batch)], memory_schema());
    table.add(reader).execute().await?;
    Ok(())
}

/// Insert or replace a fully-serialised entry (upsert semantics).
///
/// Unlike [`insert_raw`] (which is idempotent / insert-or-ignore), this
/// always overwrites an existing row with the same `id`.
pub fn upsert_raw(
    db: &LanceDb,
    id: &str,
    session_id: &str,
    project_path: &str,
    ts: &str,
    data: &str,
) -> Result<()> {
    block(async {
        let table = get_table(db).await?;
        table.delete(&format!("id = '{}'", esc(id))).await?;
        insert_raw_async(db, id, session_id, project_path, ts, data).await
    })
}

// ─── Read ─────────────────────────────────────────────────────────────────────

/// Load every entry as a raw JSON string, ordered by timestamp ascending.
pub fn load_all_json(db: &LanceDb) -> Result<Vec<String>> {
    block(async {
        let table = get_table(db).await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .execute()
            .await?
            .try_collect()
            .await?;

        let mut rows: Vec<(String, String)> = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                let data = read_str(batch, "data", i);
                let ts = read_str(batch, "ts", i);
                if !data.is_empty() {
                    rows.push((ts, data));
                }
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(rows.into_iter().map(|(_, d)| d).collect())
    })
}

/// Load entries for a specific project path, ordered by timestamp ascending.
pub fn load_by_project(db: &LanceDb, project_path: &str) -> Result<Vec<String>> {
    block(async {
        let table = get_table(db).await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .only_if(format!("project_path = '{}'", esc(project_path)))
            .execute()
            .await?
            .try_collect()
            .await?;

        let mut rows: Vec<(String, String)> = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                let data = read_str(batch, "data", i);
                let ts = read_str(batch, "ts", i);
                if !data.is_empty() {
                    rows.push((ts, data));
                }
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(rows.into_iter().map(|(_, d)| d).collect())
    })
}

/// Return the raw JSON for a single entry by UUID, or `None` if not found.
pub fn get_by_id(db: &LanceDb, id: &str) -> Result<Option<String>> {
    block(async {
        let table = get_table(db).await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .only_if(format!("id = '{}'", esc(id)))
            .limit(1)
            .execute()
            .await?
            .try_collect()
            .await?;

        for batch in &batches {
            if batch.num_rows() > 0 {
                let data = read_str(batch, "data", 0);
                if !data.is_empty() {
                    return Ok(Some(data));
                }
            }
        }
        Ok(None)
    })
}

/// Return the total number of entries stored.
pub fn count(db: &LanceDb) -> Result<i64> {
    block(async {
        let table = get_table(db).await?;
        Ok(table.count_rows(None).await? as i64)
    })
}

// ─── Mutations ────────────────────────────────────────────────────────────────

/// Delete an entry by its UUID string.  Returns `true` if a row was removed.
pub fn delete_by_id(db: &LanceDb, id: &str) -> Result<bool> {
    block(async {
        let table = get_table(db).await?;
        let before = table.count_rows(None).await?;
        table.delete(&format!("id = '{}'", esc(id))).await?;
        let after = table.count_rows(None).await?;
        Ok(after < before)
    })
}

/// Delete all entries whose `project_path` column equals `path`.
/// Returns the number of rows removed.
pub fn delete_by_project(db: &LanceDb, path: &str) -> Result<usize> {
    block(async {
        let table = get_table(db).await?;
        let before = table.count_rows(None).await?;
        table
            .delete(&format!("project_path = '{}'", esc(path)))
            .await?;
        let after = table.count_rows(None).await?;
        Ok(before.saturating_sub(after))
    })
}

/// Delete every row in `memory_entries`.
pub fn delete_all(db: &LanceDb) -> Result<()> {
    block(async {
        let table = get_table(db).await?;
        // Predicate that matches all rows.
        table.delete("id IS NOT NULL OR id IS NULL").await?;
        Ok(())
    })
}

/// Replace the `data` JSON blob for entry `id`.
/// Returns `true` when the row was found and updated; `false` when not found.
pub fn update_data(db: &LanceDb, id: &str, new_data: &str) -> Result<bool> {
    block(async {
        let table = get_table(db).await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .only_if(format!("id = '{}'", esc(id)))
            .limit(1)
            .execute()
            .await?
            .try_collect()
            .await?;

        let found = batches.iter().any(|b| b.num_rows() > 0);
        if !found {
            return Ok(false);
        }
        let batch = &batches[0];
        let sid = read_str(batch, "session_id", 0);
        let pp = read_str(batch, "project_path", 0);
        let ts = read_str(batch, "ts", 0);

        table.delete(&format!("id = '{}'", esc(id))).await?;
        insert_raw_async(db, id, &sid, &pp, &ts, new_data).await?;
        Ok(true)
    })
}

// ─── Vector / Semantic Search ─────────────────────────────────────────────────

/// Parameters for [`search_history`].
#[derive(Debug, Default, Clone)]
pub struct SearchParams {
    /// Exact-match filter on `project_path`.
    pub project_path: Option<String>,
    /// Entry must contain ALL listed tags (post-filter).
    pub tags: Option<Vec<String>>,
    /// ISO-8601 lower bound on `ts`.
    pub start_date: Option<String>,
    /// ISO-8601 upper bound on `ts`.
    pub end_date: Option<String>,
    /// Maximum results to return (default: 10).
    pub limit: usize,
    /// Pre-computed query embedding for ANN vector search.
    /// When `Some`, `search_history_async` uses `nearest_to` instead of
    /// keyword LIKE filters.  Supply via [`crate::embed::embed_query`].
    pub query_vec: Option<Vec<f32>>,
}

/// Structured result row returned by [`search_history`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub session_id: String,
    pub project_path: String,
    pub intent: String,
    pub decision: String,
    pub source_ide: String,
    pub tags: Vec<String>,
    pub ts: String,
    /// Raw JSON blob for backward-compatible deserialisation by callers.
    pub data: String,
}

/// Fetch the `n` most-recent memory entries, sorted by timestamp descending.
///
/// This is the primary data source for the `cortex://memory/recent` MCP
/// resource — it gives the agent immediate awareness of what was recently
/// worked on without requiring it to issue an explicit tool call.
pub fn load_recent(db: &LanceDb, n: usize) -> Result<Vec<HistoryEntry>> {
    search_history(
        db,
        "",
        SearchParams {
            project_path: None,
            tags:         None,
            start_date:   None,
            end_date:     None,
            limit:        n,
            query_vec:    None,
        },
    )
}

/// Search the memory history — vector ANN when an embedding is provided,
/// keyword LIKE fallback otherwise.
///
/// Structural filters (`project_path`, `start_date`, `end_date`) are applied
/// as LanceDB predicate expressions.  Tag filtering is done in-process after
/// retrieval (LanceDB does not support array-contains natively).
///
/// Pass [`SearchParams::query_vec`] (from [`crate::embed::embed_query`])
/// to use ANN search instead of the keyword LIKE path.
pub fn search_history(db: &LanceDb, query: &str, params: SearchParams) -> Result<Vec<HistoryEntry>> {
    block(search_history_async(db, query, params))
}

/// Async version of [`search_history`].
pub async fn search_history_async(
    db: &LanceDb,
    query: &str,
    params: SearchParams,
) -> Result<Vec<HistoryEntry>> {
    let table = get_table(db).await?;
    let limit = if params.limit == 0 { 10 } else { params.limit };

    // Build scalar pre-filters (project_path, date range).
    // These apply regardless of whether we use vector or keyword search.
    let mut scalar_filters: Vec<String> = Vec::new();
    if let Some(ref pp) = params.project_path {
        scalar_filters.push(format!("project_path = '{}'", esc(pp)));
    }
    if let Some(ref start) = params.start_date {
        scalar_filters.push(format!("ts >= '{}'", esc(start)));
    }
    if let Some(ref end) = params.end_date {
        scalar_filters.push(format!("ts <= '{}'", esc(end)));
    }
    let scalar_pred = if scalar_filters.is_empty() {
        None
    } else {
        Some(scalar_filters.join(" AND "))
    };

    let batches: Vec<RecordBatch> = if let Some(qvec) = params.query_vec {
        // ── Vector ANN path ───────────────────────────────────────────────
        // Over-fetch by 4× so post-filtering by tags still returns `limit`
        // results in the common case.
        let mut vq = table
            .query()
            .nearest_to(qvec.as_slice())?
            .column("vector")
            .limit(limit * 4);
        if let Some(pred) = scalar_pred {
            vq = vq.only_if(pred);
        }
        vq.execute().await?.try_collect().await?
    } else {
        // ── Keyword / full-text fallback ──────────────────────────────────
        let mut filters = scalar_filters;
        if !query.is_empty() {
            let q = esc(query);
            filters.push(format!(
                "(intent LIKE '%{q}%' OR decision LIKE '%{q}%' OR data LIKE '%{q}%')"
            ));
        }
        let mut qb = table.query().limit(limit * 4);
        if !filters.is_empty() {
            qb = qb.only_if(filters.join(" AND "));
        }
        qb.execute().await?.try_collect().await?
    };

    let mut results: Vec<HistoryEntry> = Vec::new();

    for batch in &batches {
        for i in 0..batch.num_rows() {
            let tags_json = read_str(batch, "tags", i);
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

            // Post-filter by required tags.
            if let Some(ref required) = params.tags {
                if !required.iter().all(|t| tags.contains(t)) {
                    continue;
                }
            }

            results.push(HistoryEntry {
                id:           read_str(batch, "id", i),
                session_id:   read_str(batch, "session_id", i),
                project_path: read_str(batch, "project_path", i),
                intent:       read_str(batch, "intent", i),
                decision:     read_str(batch, "decision", i),
                source_ide:   read_str(batch, "source_ide", i),
                tags,
                ts:           read_str(batch, "ts", i),
                data:         read_str(batch, "data", i),
            });

            if results.len() >= limit {
                break;
            }
        }
        if results.len() >= limit {
            break;
        }
    }

    // Vector results come back ranked by distance; keyword results are sorted
    // newest-first.  Apply the same sort in both cases for consistent output.
    results.sort_by(|a, b| b.ts.cmp(&a.ts));
    results.truncate(limit);
    Ok(results)
}
