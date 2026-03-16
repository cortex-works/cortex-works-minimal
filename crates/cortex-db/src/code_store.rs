//! # cortex-db — LanceDB Code-Node Store
//!
//! Persists AST symbol metadata alongside 512-dim embeddings so the AI agent
//! can perform hybrid (vector ANN + scalar pre-filter) code search across the
//! full indexed codebase without reading any source file at query time.
//!
//! ## Schema (`code_nodes`)
//!
//! | Column        | Type                    | Notes                                |
//! |---------------|-------------------------|--------------------------------------|
//! | id            | Utf8 (not-null)         | `{project_path}::{file_path}::{name}`|
//! | project_path  | Utf8                    | Absolute workspace root              |
//! | file_path     | Utf8                    | Relative path inside project         |
//! | symbol_name   | Utf8                    | e.g. `handle_request`                |
//! | kind          | Utf8                    | `function` / `struct` / `impl` / …  |
//! | content_hash  | Utf8                    | MD5 hex of raw symbol text           |
//! | tags          | Utf8                    | Comma-separated tags (e.g. "incomplete,mock") |
//! | vector        | FixedSizeList<f32>[512] | Passage embedding from `embed.rs`    |

use anyhow::Result;
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use futures_util::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use std::sync::Arc;

use crate::{LanceDb, block};

// ─── Constants ────────────────────────────────────────────────────────────────

const TABLE: &str = "code_nodes";
const VECTOR_DIM: i32 = 512;

// ─── Public types ─────────────────────────────────────────────────────────────

/// A single indexed AST symbol ready for persistence.
#[derive(Debug, Clone)]
pub struct CodeNode {
    /// Composite primary key: `"{project_path}::{file_path}::{symbol_name}"`.
    pub id: String,
    /// Absolute path to the workspace root (e.g. `/home/user/repos/my-app`).
    pub project_path: String,
    /// Path to the source file, relative to `project_path`.
    pub file_path: String,
    /// Symbol identifier as it appears in source code.
    pub symbol_name: String,
    /// AST node kind: `"function"`, `"struct"`, `"impl"`, `"enum"`, etc.
    pub kind: String,
    /// Fast hash (e.g. MD5-hex) of the raw symbol text.  Used to skip
    /// re-embedding nodes whose content has not changed since last index.
    pub content_hash: String,
    /// Comma-separated classification tags (e.g. `"incomplete"`, `"mock"`).
    /// Assigned by the Orphan Tracker logic in the indexer.
    pub tags: String,
    /// 512-dim passage embedding.  Pass an empty `Vec` when the caller wants
    /// the DB layer to skip updating the vector (hash-unchanged path).
    /// `upsert_code_nodes` will zero-pad any vector shorter than 512.
    pub vector: Vec<f32>,
}

/// Lightweight result row returned by [`search_code_semantic`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodeNodeRef {
    pub project_path: String,
    pub file_path: String,
    pub symbol_name: String,
    pub kind: String,
    pub content_hash: String,
    /// Comma-separated classification tags (e.g. `"incomplete"`, `"mock"`).
    pub tags: String,
    /// Cosine similarity score in [0, 1].  `1.0 - _distance` from LanceDB.
    /// Defaults to `0.0` when the ANN query does not return a `_distance` column.
    pub similarity: f32,
}

// ─── Schema ───────────────────────────────────────────────────────────────────

/// Arrow schema for the `code_nodes` LanceDB table.
pub fn code_node_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("project_path", DataType::Utf8, true),
        Field::new("file_path", DataType::Utf8, true),
        Field::new("symbol_name", DataType::Utf8, true),
        Field::new("kind", DataType::Utf8, true),
        Field::new("content_hash", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, true),
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

/// Open (or lazily create) the `code_nodes` table.
async fn get_table(db: &LanceDb) -> Result<lancedb::Table> {
    let schema = code_node_schema();
    match db.conn().open_table(TABLE).execute().await {
        Ok(t) => Ok(t),
        Err(_) => {
            let empty = RecordBatch::new_empty(schema.clone());
            let reader = RecordBatchIterator::new(vec![Ok(empty)], schema);
            Ok(db.conn().create_table(TABLE, reader).execute().await?)
        }
    }
}

/// Build a multi-row `RecordBatch` from a slice of `CodeNode`s.
fn build_batch(nodes: &[CodeNode]) -> Result<RecordBatch> {
    let schema = code_node_schema();
    let n = nodes.len();

    let ids:          Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let project_paths: Vec<&str> = nodes.iter().map(|n| n.project_path.as_str()).collect();
    let file_paths:   Vec<&str> = nodes.iter().map(|n| n.file_path.as_str()).collect();
    let symbol_names: Vec<&str> = nodes.iter().map(|n| n.symbol_name.as_str()).collect();
    let kinds:        Vec<&str> = nodes.iter().map(|n| n.kind.as_str()).collect();
    let hashes:       Vec<&str> = nodes.iter().map(|n| n.content_hash.as_str()).collect();
    let tags_col:     Vec<&str> = nodes.iter().map(|n| n.tags.as_str()).collect();

    // Flatten all 512-dim vectors into a single f32 buffer.
    let mut flat_floats: Vec<f32> = Vec::with_capacity(n * VECTOR_DIM as usize);
    for node in nodes {
        let mut v = node.vector.clone();
        v.resize(VECTOR_DIM as usize, 0.0_f32);
        flat_floats.extend_from_slice(&v);
    }

    let float_vals = Arc::new(Float32Array::from(flat_floats)) as Arc<dyn Array>;
    let vector_col = Arc::new(FixedSizeListArray::new(
        Arc::new(Field::new("item", DataType::Float32, true)),
        VECTOR_DIM,
        float_vals,
        None,
    )) as Arc<dyn Array>;

    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(ids))           as Arc<dyn Array>,
            Arc::new(StringArray::from(project_paths)) as Arc<dyn Array>,
            Arc::new(StringArray::from(file_paths))    as Arc<dyn Array>,
            Arc::new(StringArray::from(symbol_names))  as Arc<dyn Array>,
            Arc::new(StringArray::from(kinds))         as Arc<dyn Array>,
            Arc::new(StringArray::from(hashes))        as Arc<dyn Array>,
            Arc::new(StringArray::from(tags_col))      as Arc<dyn Array>,
            vector_col,
        ],
    )?)
}

/// Escape a string value for safe use inside a LanceDB SQL predicate.
fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// Read a `Utf8` column value from a `RecordBatch` row as an owned `String`.
fn read_str(batch: &RecordBatch, col: &str, idx: usize) -> String {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .map(|a| a.value(idx).to_string())
        .unwrap_or_default()
}

/// Read an `f32` from a column; returns `0.0` when missing or not Float32.
fn read_f32(batch: &RecordBatch, col: &str, idx: usize) -> f32 {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>())
        .map(|a| a.value(idx))
        .unwrap_or(0.0)
}

// ─── Write ────────────────────────────────────────────────────────────────────

/// Upsert a batch of `CodeNode`s into the `code_nodes` table.
///
/// ## Upsert semantics
/// For each node, the existing row with the same `id` (if any) is replaced
/// wholesale.  This is achieved via LanceDB's native `merge_insert`:
///
/// * `when_matched_update_all` — overwrite every column on an ID match.
/// * `when_not_matched_insert_all` — plain INSERT for new IDs.
///
/// This is cheaper than a delete+insert cycle and avoids write amplification
/// when only a handful of symbols in a file actually changed.
///
/// ## Empty batch
/// Calling with an empty slice is a **no-op** — no round-trip to the DB.
pub fn upsert_code_nodes(db: &LanceDb, nodes: &[CodeNode]) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    block(upsert_code_nodes_async(db, nodes))
}

async fn upsert_code_nodes_async(db: &LanceDb, nodes: &[CodeNode]) -> Result<()> {
    let table = get_table(db).await?;
    let batch = build_batch(nodes)?;
    let schema = code_node_schema();
    let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);

    let mut merge = table.merge_insert(&["id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    if let Err(err) = merge.execute(Box::new(reader)).await {
        let msg = err.to_string();
        let lower = msg.to_lowercase();
        let is_schema_mismatch = lower.contains("append with different schema")
            || (lower.contains("schema") && lower.contains("mismatch"))
            || (lower.contains("different schema") && lower.contains("append"));

        if is_schema_mismatch {
            eprintln!(
                "[cortex-db][CRITICAL] code_nodes schema mismatch detected during upsert. \
This usually means stale local Lance files from an older schema. \
Please stop sync processes and delete the local `.lance` directory, then restart indexing. \
Batch was skipped to avoid infinite retry loops. Error: {msg}"
            );
            return Ok(());
        }

        return Err(err.into());
    }

    Ok(())
}

/// Delete all `code_nodes` rows for the given `(project_path, file_path)` pair.
///
/// Call this before re-indexing a file to remove stale symbols that were
/// deleted or renamed since the last scan.
pub fn delete_file_nodes(db: &LanceDb, project_path: &str, file_path: &str) -> Result<()> {
    block(async {
        let table = get_table(db).await?;
        table
            .delete(&format!(
                "project_path = '{}' AND file_path = '{}'",
                esc(project_path),
                esc(file_path),
            ))
            .await?;
        Ok(())
    })
}

/// Purge ALL `code_nodes` rows for `project_path` whose `file_path` starts
/// with a build-artefact or dependency directory prefix.
///
/// Returns the number of rows deleted (may not be exact on all LanceDB
/// versions — treat as "at least N were matched").
///
/// Excluded prefixes (relative to project root):
/// `target/`, `node_modules/`, `dist/`, `build/`, `out/`, `.git/`
pub fn purge_excluded_file_paths(db: &LanceDb, project_path: &str) -> Result<u64> {
    const EXCLUDED_PREFIXES: &[&str] = &[
        "target/", "node_modules/", "dist/", "build/", "out/", ".git/",
        // Also catch Windows-style path separators that survive normalisation.
        "target\\", "node_modules\\", "dist\\", "build\\", "out\\", ".git\\",
    ];

    block(async {
        let table = get_table(db).await?;

        // Build a SQL predicate that matches any of the excluded prefixes
        // for this specific project_path.
        let proj_escaped = esc(project_path);
        let prefix_clauses: Vec<String> = EXCLUDED_PREFIXES
            .iter()
            .map(|pfx| format!("file_path LIKE '{}%'", pfx))
            .collect();
        let filter = format!(
            "project_path = '{}' AND ({})",
            proj_escaped,
            prefix_clauses.join(" OR ")
        );

        // Count rows first so we can report how many were removed.
        let before: u64 = table
            .query()
            .only_if(filter.as_str())
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?
            .iter()
            .map(|b| b.num_rows() as u64)
            .sum();

        if before > 0 {
            table.delete(&filter).await?;
        }

        Ok(before)
    })
}

// ─── Read ─────────────────────────────────────────────────────────────────────

/// Look up the `content_hash` for a single symbol by its composite `id`.
///
/// Returns `None` when the symbol is not yet indexed.  The background indexer
/// uses this to skip re-embedding symbols whose content hasn't changed.
pub fn get_content_hash(db: &LanceDb, id: &str) -> Result<Option<String>> {
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
                return Ok(Some(read_str(batch, "content_hash", 0)));
            }
        }
        Ok(None)
    })
}

/// **Semantic code search** — find the `limit` most relevant symbols for a
/// pre-computed query vector, optionally scoped to one project.
///
/// Returns lightweight [`CodeNodeRef`] rows (no vector blob) suitable for
/// formatting as Markdown symbol references in an agent response.
pub fn search_code_semantic(
    db: &LanceDb,
    query_vec: Vec<f32>,
    project_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<CodeNodeRef>> {
    block(search_code_semantic_async(db, query_vec, project_filter, limit))
}

// ─── Stats ────────────────────────────────────────────────────────────────────

/// Per-project index statistics returned by [`get_index_stats`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexStats {
    /// Absolute workspace root.
    pub project_path: String,
    /// Number of indexed source files (sentinel rows excluded).
    pub file_count: usize,
    /// Number of named symbols (`function`, `struct`, `class`, etc.).
    pub symbol_count: usize,
}

/// Return symbol-count and file-count statistics for every indexed project.
///
/// Queries only the non-sentinel rows (`kind != 'file'`) and aggregates in
/// Rust — no vector data is consumed.  Returns one [`IndexStats`] per unique
/// `project_path`, sorted by `symbol_count` descending.
pub fn get_index_stats(db: &LanceDb) -> Result<Vec<IndexStats>> {
    block(async {
        let table = get_table(db).await?;

        let batches: Vec<RecordBatch> = table
            .query()
            .only_if("kind != 'file'")
            .execute()
            .await?
            .try_collect()
            .await?;

        // Aggregate: project_path → (file set, symbol count)
        let mut acc: std::collections::HashMap<
            String,
            (std::collections::HashSet<String>, usize),
        > = std::collections::HashMap::new();

        for batch in &batches {
            for i in 0..batch.num_rows() {
                let proj = read_str(batch, "project_path", i);
                let file = read_str(batch, "file_path", i);
                let entry = acc.entry(proj).or_default();
                entry.0.insert(file);
                entry.1 += 1;
            }
        }

        let mut stats: Vec<IndexStats> = acc
            .into_iter()
            .map(|(project_path, (files, symbol_count))| IndexStats {
                project_path,
                file_count: files.len(),
                symbol_count,
            })
            .collect();

        // Most indexed projects first.
        stats.sort_by(|a, b| b.symbol_count.cmp(&a.symbol_count));
        Ok(stats)
    })
}

async fn search_code_semantic_async(
    db: &LanceDb,
    query_vec: Vec<f32>,
    project_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<CodeNodeRef>> {
    let table = get_table(db).await?;
    let limit = if limit == 0 { 5 } else { limit };

    let mut vq = table
        .query()
        .nearest_to(query_vec.as_slice())?
        .column("vector")
        .limit(limit);

    if let Some(proj) = project_filter {
        vq = vq.only_if(format!("project_path = '{}'", esc(proj)));
    }

    let batches: Vec<RecordBatch> = vq.execute().await?.try_collect().await?;

    let mut results = Vec::new();
    'outer: for batch in &batches {
        for i in 0..batch.num_rows() {
            // LanceDB ANN queries add `_distance` (cosine distance ∈ [0, 2]).
            // Convert to cosine similarity: similarity = 1.0 - distance.
            let distance   = read_f32(batch, "_distance", i);
            let similarity = (1.0_f32 - distance).clamp(0.0, 1.0);
            results.push(CodeNodeRef {
                project_path: read_str(batch, "project_path", i),
                file_path:    read_str(batch, "file_path", i),
                symbol_name:  read_str(batch, "symbol_name", i),
                kind:         read_str(batch, "kind", i),
                content_hash: read_str(batch, "content_hash", i),
                tags:         read_str(batch, "tags", i),
                similarity,
            });
            if results.len() >= limit {
                break 'outer;
            }
        }
    }
    Ok(results)
}
