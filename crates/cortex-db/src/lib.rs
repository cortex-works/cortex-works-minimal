//! # cortex-db
//!
//! Unified LanceDB data layer for the Cortex-Works monorepo.
//!
//! The minimal branch currently exposes checkpoint persistence helpers
//! used by Chronos and related workflows.

pub mod checkpoint_store;

use anyhow::Result;
use dirs::home_dir;
use std::{path::PathBuf, sync::Arc};

// ─────────────────────────────────────────────────────────────────────────────
// LanceDb — connection handle
// ─────────────────────────────────────────────────────────────────────────────

/// Shared LanceDB connection handle.
///
/// Cheap to clone — the inner `Arc` means all clones share the same
/// live connection.  `open_default_sync` is the entry-point for sync
/// call-sites; `open_default` is available for async callers.
#[derive(Clone)]
pub struct LanceDb {
    pub(crate) conn: Arc<lancedb::Connection>,
}

impl LanceDb {
    /// Open (or create) the default Cortex-Works LanceDB store at
    /// `~/.cortexast/data/cortex_lance/`.
    pub async fn open_default() -> Result<Self> {
        let path = db_path("cortex_lance");
        Self::open_path(path).await
    }

    /// Open a LanceDB at an arbitrary directory path.
    pub async fn open_path(path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&path)?;
        let conn = lancedb::connect(path.to_string_lossy().as_ref())
            .execute()
            .await?;
        Ok(Self {
            conn: Arc::new(conn),
        })
    }

    /// Blocking version of [`open_default`] — safe to call from **both**
    /// async (multi-thread Tokio) and synchronous (no runtime) contexts.
    pub fn open_default_sync() -> Result<Self> {
        block(Self::open_default())
    }

    /// Blocking open at an arbitrary path — convenience for tests and
    /// sync call-sites that need an isolated store.
    pub fn open_sync(path: impl Into<PathBuf>) -> Result<Self> {
        block(Self::open_path(path.into()))
    }

    /// Access the raw LanceDB connection for advanced operations.
    pub fn conn(&self) -> &lancedb::Connection {
        &self.conn
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// block() — sync / async bridge
// ─────────────────────────────────────────────────────────────────────────────

/// Run an async future to completion regardless of the calling context.
///
/// | Context                            | Mechanism                          |
/// |------------------------------------|------------------------------------|
/// | Inside multi-thread Tokio runtime  | `block_in_place` + handle.block_on |
/// | Outside any Tokio runtime          | Fresh `current_thread` runtime     |
pub(crate) fn block<F>(f: F) -> F::Output
where
    F: std::future::Future,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(f)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("cortex-db: failed to build Tokio runtime")
            .block_on(f),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn db_path(name: &str) -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".cortexast")
        .join("data")
        .join(name)
}
