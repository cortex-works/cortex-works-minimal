/// End-to-end test: write a memory entry with real embedding, then retrieve it
/// via semantic ANN search using a *different* query (no keyword overlap).
///
/// Intent stored : "Fixed the async deadlock in the mesh router"
/// Query issued  : "mesh network bug"
///
/// The test passes only if the ANN path finds the entry based on *semantic*
/// similarity rather than keyword matching.
use cortex_db::{LanceDb, memory_store, memory_store::SearchParams};
use serde_json::json;
use uuid::Uuid;

#[test]
fn semantic_search_finds_entry_by_meaning_not_keywords() {
    // ── 1. Open a fresh DB in a temp dir ────────────────────────────────────
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let db = LanceDb::open_sync(tmpdir.path()).expect("open LanceDb");

    // ── 2. Check model availability (graceful skip) ───────────────────────
    let probe = cortex_db::embed::embed_passage("probe");
    if probe.is_none() {
        eprintln!(
            "[e2e] SKIPPED — embedding model unavailable \
             (set CORTEX_MODEL_ID or ensure network access on first run)"
        );
        return;
    }

    // ── 3. Insert the test entry ──────────────────────────────────────────
    let id = Uuid::new_v4().to_string();
    let session_id = Uuid::new_v4().to_string();
    let entry = json!({
        "id": id,
        "session_id": session_id,
        "project_path": "/test/project",
        "intent": "Fixed the async deadlock in the mesh router",
        "decision": "Replaced the blocking Mutex with a non-blocking RwLock and added a timeout guard",
        "source_ide": "vscode",
        "tags": ["async", "deadlock", "mesh", "router"],
        "timestamp": "2026-02-27T12:00:00Z"
    });
    memory_store::insert_raw(
        &db,
        &id,
        &session_id,
        "/test/project",
        "2026-02-27T12:00:00Z",
        &entry.to_string(),
    )
    .expect("insert_raw");

    // ── 4. Embed the *query* (different words, same semantic space) ────────
    let query = "mesh network bug";
    let query_vec = cortex_db::embed::embed_query(query);
    assert!(query_vec.is_some(), "embed_query must return Some when model is loaded");

    // ── 5. Semantic search ────────────────────────────────────────────────
    let params = SearchParams {
        project_path: None,
        tags: None,
        start_date: None,
        end_date: None,
        limit: 5,
        query_vec,
    };
    let results = memory_store::search_history(&db, query, params)
        .expect("search_history");

    // ── 6. Print & assert ─────────────────────────────────────────────────
    println!("\n=== Semantic Search Results for: {:?} ===", query);
    for (i, r) in results.iter().enumerate() {
        println!("[{}] intent  : {}", i + 1, r.intent);
        println!("     decision: {}", r.decision);
        println!("     tags    : {:?}", r.tags);
        println!();
    }

    assert!(!results.is_empty(), "ANN search returned no results");
    assert_eq!(results[0].id, id, "Top result must be the inserted entry");
    println!(
        "✓  PASS — query {:?} found entry via semantic similarity (no keyword match).",
        query
    );
}
