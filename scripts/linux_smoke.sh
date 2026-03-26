#!/usr/bin/env bash
# scripts/linux_smoke.sh — Fast Linux / CI smoke gate
#
# Runs the two critical integration tests that cover the full 13-tool MCP
# surface in ~2 seconds.  Use this instead of `cargo test --workspace` for
# fast iteration on Linux or in CI pipelines.
#
# Usage:
#   bash scripts/linux_smoke.sh            # default (concise pass/fail)
#   bash scripts/linux_smoke.sh --verbose  # stream all test output
#
# Exit code:
#   0  — both tests passed
#   1  — one or more tests failed
# =============================================================================
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERBOSE=false
[[ "${1:-}" == "--verbose" ]] && VERBOSE=true

pass()  { printf "\033[32m✅  %s\033[0m\n" "$*"; }
fail()  { printf "\033[31m❌  %s\033[0m\n" "$*" >&2; }
info()  { printf "\033[34m──  %s\033[0m\n" "$*"; }

CARGO_FLAGS=()
if $VERBOSE; then
    CARGO_FLAGS+=("--" "--nocapture")
fi

info "cortex-works-minimal Linux smoke gate"
info "Repo: $REPO_ROOT"
echo

cd "$REPO_ROOT"

EXIT=0

info "1/2  cortex-ast MCP stdio smoke (cortexast crate)"
if cargo test -p cortexast mcp_stdio_smoke --quiet "${CARGO_FLAGS[@]}" 2>&1; then
    pass "mcp_stdio_smoke passed"
else
    fail "mcp_stdio_smoke FAILED"
    EXIT=1
fi

echo

info "2/2  cortex-mcp full 13-tool stack smoke (release build)"
if cargo test -p cortex-mcp full_tool_smoke_and_hot_reload "${CARGO_FLAGS[@]}" 2>&1; then
    pass "full_tool_smoke_and_hot_reload passed"
else
    fail "full_tool_smoke_and_hot_reload FAILED"
    EXIT=1
fi

echo
if [[ $EXIT -eq 0 ]]; then
    pass "All smoke tests passed — MCP surface verified on Linux"
else
    fail "One or more smoke tests failed — see output above"
fi

exit $EXIT
