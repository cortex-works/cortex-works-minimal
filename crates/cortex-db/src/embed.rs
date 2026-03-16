//! # cortex-db — Query Embedder
//!
//! Provides a **process-global** lazy embedding function for turning retrieval
//! queries into 512-dimensional f32 vectors, backed by
//! `model2vec-rs` (`minishlab/potion-retrieval-32M`).
//!
//! ## Usage
//! ```no_run
//! let vec: Option<Vec<f32>> = cortex_db::embed::embed_query("refactor login handler");
//! ```
//!
//! ## Design
//! * The `StaticModel` is loaded **once** on first call via `OnceLock`.
//! * If the model fails to load (network unavailable, bad env var), the
//!   function returns `None` so callers can fall back to keyword search.
//! * The query is prefixed with `"query: "` — the asymmetric counterpart
//!   to the `"passage: "` prefix used when *storing* embedded records in
//!   the local semantic index. This matches the retrieval conventions of
//!   `potion-retrieval-32M`.
//!
//! ## Model ID
//! Defaults to `"minishlab/potion-retrieval-32M"`.
//! Override with the env var `CORTEX_MODEL_ID` (same key used by cortex-act).

use model2vec_rs::model::StaticModel;
use std::sync::OnceLock;

// ─── Constants ────────────────────────────────────────────────────────────────

/// Default HuggingFace Hub model ID. Must match the model used at write-time
/// for semantic code indexing so that query and document vectors are in the same space.
pub const DEFAULT_MODEL_ID: &str = "minishlab/potion-retrieval-32M";

/// Expected output dimension from the default model.
pub const VECTOR_DIM: usize = 512;

// ─── Internal state ───────────────────────────────────────────────────────────

struct CachedEmbedder {
    model: StaticModel,
}

/// Process-global lazy embedder.  `None` ⟹ model failed to load.
static EMBEDDER: OnceLock<Option<CachedEmbedder>> = OnceLock::new();

fn get_embedder() -> Option<&'static CachedEmbedder> {
    EMBEDDER
        .get_or_init(|| {
            let model_id = std::env::var("CORTEX_MODEL_ID")
                .unwrap_or_else(|_| DEFAULT_MODEL_ID.to_string());

            match StaticModel::from_pretrained(&model_id, None, None, None) {
                Ok(model) => {
                    tracing::info!(model_id, "cortex-db: embedding model loaded");
                    Some(CachedEmbedder { model })
                }
                Err(e) => {
                    tracing::warn!(
                        model_id,
                        error = %e,
                        "cortex-db: embedding model failed to load — \
                         falling back to keyword search"
                    );
                    None
                }
            }
        })
        .as_ref()
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Embed `text` into a 512-dim f32 vector suitable for LanceDB ANN search.
///
/// Returns `None` when the embedding model is unavailable (first-run download
/// failed, no internet, bad env var). In that case the caller should gracefully
/// degrade to keyword/full-text search.
///
/// The input is prefixed with `"query: "` to match the asymmetric retrieval
/// convention of `potion-retrieval-32M` (documents use `"passage: "`).
pub fn embed_query(text: &str) -> Option<Vec<f32>> {
    if text.trim().is_empty() {
        return None;
    }
    let emb = get_embedder()?;
    let prefixed = format!("query: {text}");
    let vec = emb.model.encode_single(&prefixed);
    Some(vec)
}

/// Embed `text` for **storage** using the `"passage: "` prefix.
///
/// Called by `memory_store::insert_raw_async` to generate document-side
/// vectors at write time. Using a distinct prefix from [`embed_query`]
/// mirrors the asymmetric retrieval convention of `potion-retrieval-32M`
/// (queries use `"query: "`).
pub fn embed_passage(text: &str) -> Option<Vec<f32>> {
    if text.trim().is_empty() {
        return None;
    }
    let emb = get_embedder()?;
    let prefixed = format!("passage: {text}");
    let vec = emb.model.encode_single(&prefixed);
    Some(vec)
}

/// Build an **Enriched Semantic String** from AST node metadata and embed it
/// as a `"passage: "` document vector for storage in `code_nodes`.
///
/// The enriched string format is:
/// ```text
/// Project: {project} | File: {file} | {kind}: {name} | Docs: {docs} | Signature: {signature}
/// ```
///
/// Captures enough context for the model to distinguish a struct field in
/// `auth/session.rs` from an identically-named function in `net/router.rs`.
///
/// Returns `None` when the embedding model is unavailable — callers should
/// store a zero vector and re-embed on the next warm boot.
pub fn embed_code_node(
    project: &str,
    file: &str,
    kind: &str,
    name: &str,
    docs: &str,
    signature: &str,
) -> Option<Vec<f32>> {
    let text = format!(
        "Project: {project} | File: {file} | {kind}: {name} | Docs: {docs} | Signature: {signature}"
    );
    embed_passage(&text)
}
