//! # cortex-db — LanceDB Vector Checkpoint Store
//!
//! Stores reusable logic blueprints in an **isolated** `agent_checkpoints`
//! LanceDB table.  Strictly separated from `code_nodes`, `memory_entries`,
//! `agent_rules`, and `project_tracking` to prevent semantic cross-contamination
//! during ANN search.
//!
//! ## Schema (`agent_checkpoints`)
//!
//! | Column      | Type                    | Notes                           |
//! |-------------|-------------------------|---------------------------------|
//! | id          | Utf8                    | UUID v4 primary key             |
//! | concept_key | Utf8                    | Unique blueprint identifier     |
//! | core_logic  | Utf8                    | Dense compressed logic/config   |
//! | tags        | Utf8 (JSON array)       | Serialised `Vec<String>`        |
//! | ts          | Utf8                    | RFC 3339 timestamp              |
//! | vector      | FixedSizeList<f32>[512] | Embedding of `core_logic`       |
//!
//! ## Upsert contract
//! `upsert_checkpoint` deletes any existing row whose `concept_key` matches
//! before inserting the new record.  This prevents stale blueprints from
//! accumulating over time while keeping the operation idempotent.

use anyhow::Result;
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures_util::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;
use uuid::Uuid;

use crate::{LanceDb, block};

// ─── Table configuration ──────────────────────────────────────────────────────

const TABLE: &str = "agent_checkpoints";
const VECTOR_DIM: i32 = 512;

// ─── Public types ─────────────────────────────────────────────────────────────

/// A single checkpoint returned by [`search_checkpoints`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CheckpointEntry {
    pub id: String,
    pub concept_key: String,
    pub core_logic: String,
    pub tags: Vec<String>,
    pub ts: String,
}

// ─── Schema ───────────────────────────────────────────────────────────────────

fn checkpoint_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id",          DataType::Utf8, false),
        Field::new("concept_key", DataType::Utf8, true),
        Field::new("core_logic",  DataType::Utf8, true),
        Field::new("tags",        DataType::Utf8, true),
        Field::new("ts",          DataType::Utf8, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                VECTOR_DIM,
            ),
            true,
        ),
    ]))
}

// ─── Internal helpers ─────────────────────────────────────────────────────────

/// Get or lazily create the `agent_checkpoints` table.
async fn get_table(db: &LanceDb) -> Result<lancedb::Table> {
    let schema = checkpoint_schema();
    match db.conn().open_table(TABLE).execute().await {
        Ok(t) => Ok(t),
        Err(_) => {
            let empty = RecordBatch::new_empty(schema.clone());
            let reader = RecordBatchIterator::new(vec![Ok(empty)], schema);
            Ok(db.conn().create_table(TABLE, reader).execute().await?)
        }
    }
}

/// Build a single-row `RecordBatch` for the `agent_checkpoints` schema.
fn build_batch(
    id: &str,
    concept_key: &str,
    core_logic: &str,
    tags_json: &str,
    ts: &str,
    vector: Vec<f32>,
) -> Result<RecordBatch> {
    let schema = checkpoint_schema();
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
            Arc::new(StringArray::from(vec![id]))          as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![concept_key])) as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![core_logic]))  as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![tags_json]))   as Arc<dyn Array>,
            Arc::new(StringArray::from(vec![ts]))          as Arc<dyn Array>,
            vector_col,
        ],
    )?)
}

/// Read a `Utf8` column value from a `RecordBatch` row.
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

/// Lightweight deterministic embedding used in no-network builds.
///
/// This preserves vector-search code paths without relying on external models.
fn deterministic_embedding(text: &str) -> Vec<f32> {
    let mut out = vec![0.0_f32; VECTOR_DIM as usize];
    if text.is_empty() {
        return out;
    }

    for (i, b) in text.bytes().enumerate() {
        let idx = i % out.len();
        out[idx] += (b as f32) / 255.0;
    }

    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut out {
            *v /= norm;
        }
    }
    out
}

// ─── Write ────────────────────────────────────────────────────────────────────

/// Upsert a checkpoint.
///
/// Deletes any existing row whose `concept_key` matches the supplied value,
/// then inserts a fresh record with a new UUID and updated timestamp.
pub fn upsert_checkpoint(
    db: &LanceDb,
    concept_key: &str,
    core_logic: &str,
    tags: &[String],
) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let ts = chrono::Utc::now().to_rfc3339();
    let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".to_string());
    let vec = deterministic_embedding(core_logic);

    let id_owned = id.clone();
    let ck = concept_key.to_string();
    let cl = core_logic.to_string();

    block(async move {
        let table = get_table(db).await?;

        // Delete any existing row with the same concept_key (upsert semantics).
        let filter = format!("concept_key = '{}'", esc(&ck));
        table.delete(&filter).await?;

        // Insert the new row.
        let schema = checkpoint_schema();
        let batch = build_batch(&id_owned, &ck, &cl, &tags_json, &ts, vec)?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(reader).execute().await?;

        Ok(id_owned)
    })
}

// ─── Read ─────────────────────────────────────────────────────────────────────

/// Search checkpoints.
///
/// When `query_vec` is `Some`, performs a vector ANN search (cosine nearest
/// neighbour) against the `vector` column for the top `limit` results.
/// Falls back to keyword LIKE search when no vector is provided (embedding
/// model unavailable or empty query).
pub fn search_checkpoints(
    db: &LanceDb,
    query: &str,
    query_vec: Option<Vec<f32>>,
    limit: usize,
) -> Result<Vec<CheckpointEntry>> {
    block(async move {
        let table = get_table(db).await?;
        let lim = if limit == 0 { 3 } else { limit };

        let batches: Vec<RecordBatch> = if let Some(qvec) = query_vec {
            // ── Vector ANN path ───────────────────────────────────────────
            table
                .query()
                .nearest_to(qvec.as_slice())?
                .column("vector")
                .limit(lim)
                .execute()
                .await?
                .try_collect()
                .await?
        } else {
            // ── Keyword fallback ──────────────────────────────────────────
            let q = esc(query);
            let filter = format!(
                "(concept_key LIKE '%{q}%' OR core_logic LIKE '%{q}%' OR tags LIKE '%{q}%')"
            );
            table
                .query()
                .only_if(filter)
                .limit(lim)
                .execute()
                .await?
                .try_collect()
                .await?
        };

        let mut results = Vec::new();
        for batch in &batches {
            for i in 0..batch.num_rows() {
                let tags_json = read_str(batch, "tags", i);
                let tags: Vec<String> =
                    serde_json::from_str(&tags_json).unwrap_or_default();
                results.push(CheckpointEntry {
                    id:          read_str(batch, "id", i),
                    concept_key: read_str(batch, "concept_key", i),
                    core_logic:  read_str(batch, "core_logic", i),
                    tags,
                    ts:          read_str(batch, "ts", i),
                });
                if results.len() >= lim {
                    break;
                }
            }
            if results.len() >= lim {
                break;
            }
        }

        Ok(results)
    })
}
