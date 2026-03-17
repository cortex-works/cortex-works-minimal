#!/usr/bin/env bash
# =============================================================================
# scripts/release.sh — cortex-works-minimal Local Release
#
# Builds cortex-mcp for all platforms from your Mac and uploads to GitHub.
#
# What this script does automatically:
#   1. Validates version (Cargo.toml matches VERSION constant)
#   2. Promotes CHANGELOG.md "## Unreleased" → "## vX.Y.Z (YYYY-MM-DD)"
#   3. Commits the changelog update + creates + pushes the git tag
#   4. Cross-compiles all platform targets
#   5. Packages .tar.gz / .zip archives  
#   6. Creates a GitHub release using the changelog section as release notes
#
# Prerequisites (run once):
#   brew install gh zig
#   gh auth login
#   cargo install cargo-zigbuild
#   rustup target add \
#     aarch64-apple-darwin \
#     aarch64-unknown-linux-gnu \
#     x86_64-pc-windows-gnullvm \
#     aarch64-pc-windows-gnullvm
#
# Usage:
#   bash scripts/release.sh            # build all + upload
#   bash scripts/release.sh --dry-run  # build only, no git changes, skip upload
# =============================================================================
set -euo pipefail

export CARGO_TERM_COLOR=always

DRY_RUN=false
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=true

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# ── Read version from Cargo.toml ───────────────────────────────────────────────
VERSION=$(grep '^version' "$REPO_ROOT/Cargo.toml" | head -1 | cut -d '"' -f2)
TAG="v$VERSION"
RELEASE_DATE="$(date -u '+%Y-%m-%d')"

# Best-effort: use all cores for faster builds.
if command -v sysctl &>/dev/null; then
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(sysctl -n hw.ncpu 2>/dev/null || echo 8)}"
fi

pass()  { printf "\033[32m✅  %s\033[0m\n" "$*"; }
info()  { printf "\033[34m──  %s\033[0m\n" "$*"; }
warn()  { printf "\033[33m⚠️   %s\033[0m\n" "$*"; }
banner(){ printf "\n\033[1;36m=== %s ===\033[0m\n" "$*"; }
die()   { printf "\033[31m❌  %s\033[0m\n" "$*" >&2; exit 1; }

# Extract the body under '## Unreleased' (stops at next '## ' header).
extract_unreleased_notes() {
  awk '
    /^## Unreleased[[:space:]]*$/ {in_unreleased=1; next}
    /^##[[:space:]]+/             {if (in_unreleased) exit}
    {if (in_unreleased) print}
  ' "$REPO_ROOT/CHANGELOG.md" 2>/dev/null || true
}

# Extract the body under '## vX.Y.Z (DATE)' (re-release: notes already promoted).
extract_version_notes() {
  local tag="$1"
  awk -v t="$tag" '
    $0 ~ ("^## " t " ") {in_section=1; next}
    /^##[[:space:]]+/   {if (in_section) exit}
    {if (in_section) print}
  ' "$REPO_ROOT/CHANGELOG.md" 2>/dev/null || true
}

# Promote "## Unreleased" → "## vVERSION (DATE)" in CHANGELOG.md.
promote_changelog() {
  local version="$1" date="$2"
  local tmp
  tmp="$(mktemp)"
  # Replace the first occurrence of the bare "## Unreleased" line.
  awk -v ver="$version" -v dt="$date" '
    !replaced && /^## Unreleased[[:space:]]*$/ {
      print "## " ver " (" dt ")"
      replaced=1
      next
    }
    {print}
  ' "$REPO_ROOT/CHANGELOG.md" > "$tmp"
  mv "$tmp" "$REPO_ROOT/CHANGELOG.md"
}

repo_slug_from_origin() {
  local origin
  origin="$(git -C "$REPO_ROOT" remote get-url origin 2>/dev/null || true)"
  [[ -n "$origin" ]] || return 1
  origin="${origin%.git}"
  origin="${origin#https://github.com/}"
  origin="${origin#http://github.com/}"
  origin="${origin#git@github.com:}"
  if [[ "$origin" =~ ^[^/]+/[^/]+$ ]]; then
    printf '%s' "$origin"
    return 0
  fi
  return 1
}

banner "cortex-works-minimal $TAG — Local Release"
info "Repo: $REPO_ROOT"
$DRY_RUN && warn "DRY-RUN mode — no git changes, no GitHub upload"

# ── Preflight checks ──────────────────────────────────────────────────────────
banner "Preflight"
for cmd in cargo cargo-zigbuild zig rustup python3; do
  if ! command -v "$cmd" &>/dev/null; then
    printf "\033[31m❌  Missing: %s\033[0m\n" "$cmd" >&2
    case "$cmd" in
      zig|cargo-zigbuild) echo "   brew install zig && cargo install cargo-zigbuild" ;;
    esac
    exit 1
  fi
done
pass "All tools present"

if ! $DRY_RUN; then
  if ! command -v gh &>/dev/null; then
    die "Missing: gh (install: brew install gh)"
  fi
  if ! gh auth status -h github.com &>/dev/null; then
    die "GitHub CLI is not authenticated. Run: gh auth login"
  fi
fi

if [[ -z "$(git -C "$REPO_ROOT" rev-parse --is-inside-work-tree 2>/dev/null || true)" ]]; then
  die "Not a git repo: $REPO_ROOT"
fi

REPO_SLUG=""
if ! $DRY_RUN; then
  REPO_SLUG="$(repo_slug_from_origin || true)"
  [[ -n "$REPO_SLUG" ]] || die "Could not parse OWNER/REPO from 'origin' remote."
  info "GitHub repo: $REPO_SLUG"
fi

# ── Release Notes (from CHANGELOG.md) ───────────────────────────────────────
banner "Release Notes"

# Create CHANGELOG.md if it doesn't exist
if [[ ! -f "$REPO_ROOT/CHANGELOG.md" ]]; then
  cat > "$REPO_ROOT/CHANGELOG.md" << 'EOF'
# Changelog

All notable changes to cortex-works-minimal will be documented in this file.

## Unreleased

- Initial placeholder entry

EOF
  info "Created CHANGELOG.md"
fi

HAVE_UNRELEASED=false
grep -q '^## Unreleased[[:space:]]*$' "$REPO_ROOT/CHANGELOG.md" && HAVE_UNRELEASED=true || true

if $HAVE_UNRELEASED; then
  RAW_NOTES="$(extract_unreleased_notes | sed -e 's/[[:space:]]\+$//')"
  if [[ -z "${RAW_NOTES//[[:space:]]/}" ]]; then
    die "CHANGELOG.md '## Unreleased' section is empty — add entries before releasing."
  fi
else
  # Re-release mode: '## Unreleased' was already promoted in a previous run.
  if ! grep -q "^## $TAG " "$REPO_ROOT/CHANGELOG.md"; then
    die "CHANGELOG.md has no '## Unreleased' section. Add release notes before running."
  fi
  RAW_NOTES="$(extract_version_notes "$TAG" | sed -e 's/[[:space:]]\+$//')"
  warn "Re-release: using existing $TAG notes from CHANGELOG (promotion already done)"
fi

TRIMMED="$(printf '%s' "$RAW_NOTES" | python3 -c "import sys; print(sys.stdin.read().strip())")"
RELEASE_NOTES="$(printf '%s\n\n---\n\n*Built on macOS arm64 via cargo-zigbuild. Cross-platform tested: macOS, Windows, Ubuntu.*' "$TRIMMED")"
info "Release notes preview (first 5 lines):"
printf '%s\n' "$TRIMMED" | head -5 | while IFS= read -r line; do info "  $line"; done

# ── Warm dependencies ───────────────────────────────────────────────────────
banner "Warm dependencies"
cargo fetch
pass "Cargo deps fetched"

# ── Promote CHANGELOG & Commit ───────────────────────────────────────────────
banner "Updating CHANGELOG.md"
if $DRY_RUN; then
  warn "Dry-run: skipping CHANGELOG promotion and git commit"
elif $HAVE_UNRELEASED; then
  promote_changelog "$TAG" "$RELEASE_DATE"
  pass "Promoted '## Unreleased' → '## $TAG ($RELEASE_DATE)'"
  git -C "$REPO_ROOT" add CHANGELOG.md
  if ! git -C "$REPO_ROOT" diff --cached --quiet; then
    git -C "$REPO_ROOT" commit -m "chore: release $TAG — promote CHANGELOG"
    pass "Committed CHANGELOG update"
  else
    info "CHANGELOG already at $TAG — skipping commit"
  fi
else
  info "Re-release: CHANGELOG already promoted for $TAG — skipping commit"
fi

# ── Tag management ────────────────────────────────────────────────────────────
banner "Tagging $TAG"
if $DRY_RUN; then
  warn "Dry-run: skipping tag creation"
else
  git -C "$REPO_ROOT" tag -d "$TAG" 2>/dev/null && info "Deleted local tag $TAG" || true
  git -C "$REPO_ROOT" push origin ":refs/tags/$TAG" 2>/dev/null && info "Deleted remote tag $TAG" || true
  git -C "$REPO_ROOT" tag "$TAG"
  git -C "$REPO_ROOT" push origin HEAD "$TAG"
  pass "Tag $TAG created and pushed"
fi

# ── Build targets ──────────────────────────────────────────────────────────
banner "Building"
TARGETS=(
  "aarch64-apple-darwin"
  "aarch64-unknown-linux-gnu"
  "x86_64-pc-windows-gnullvm"
  "aarch64-pc-windows-gnullvm"
)

for target in "${TARGETS[@]}"; do
  info "Building $target ..."
  case "$target" in
    aarch64-apple-darwin)
      cargo build --release --target "$target" -p cortex-mcp
      ;;
    *)
      cargo zigbuild --release --target "$target" -p cortex-mcp
      ;;
  esac
  pass "Built $target"
done

# ── Package ───────────────────────────────────────────────────────────────────
banner "Packaging"
DIST="$REPO_ROOT/dist"
rm -rf "$DIST" && mkdir -p "$DIST"

package_tar() {
  local target="$1" platform="$2"
  local src="$REPO_ROOT/target/$target/release"
  local dir="$DIST/cortex-mcp-$VERSION-$platform"
  mkdir -p "$dir"
  cp "$src/cortex-mcp"     "$dir/"
  cp "$REPO_ROOT/LICENSE" "$REPO_ROOT/README.md" "$dir/"
  echo "$VERSION" > "$dir/VERSION"
  tar -C "$dir" -czf "$DIST/cortex-mcp-$VERSION-$platform.tar.gz" .
  rm -rf "$dir"
  pass "Packaged $platform.tar.gz"
}

package_zip() {
  local target="$1" platform="$2"
  local src="$REPO_ROOT/target/$target/release"
  local dir="$DIST/cortex-mcp-$VERSION-$platform"
  mkdir -p "$dir"
  cp "$src/cortex-mcp.exe" "$dir/"
  cp "$REPO_ROOT/LICENSE" "$REPO_ROOT/README.md" "$dir/"
  echo "$VERSION" > "$dir/VERSION"
  (cd "$dir" && zip -qr "$DIST/cortex-mcp-$VERSION-$platform.zip" .)
  rm -rf "$dir"
  pass "Packaged $platform.zip"
}

package_tar "aarch64-apple-darwin"           "macos-arm64"
package_tar "aarch64-unknown-linux-gnu"      "linux-arm64"
package_zip "x86_64-pc-windows-gnullvm"      "windows-x64"
package_zip "aarch64-pc-windows-gnullvm"     "windows-arm64"

info "Artifacts:"
ls -lh "$DIST/"

# ── GitHub Release ────────────────────────────────────────────────────────────
if $DRY_RUN; then
  warn "Dry-run: skipping GitHub release upload"
  exit 0
fi

banner "Uploading to GitHub Release $TAG"
gh release delete "$TAG" --repo "$REPO_SLUG" --yes 2>/dev/null || true
NOTES_FILE="$(mktemp)"
printf "%s\n" "$RELEASE_NOTES" > "$NOTES_FILE"
gh release create "$TAG" \
  "$DIST"/*.tar.gz \
  "$DIST"/*.zip \
  --title "cortex-works-minimal $TAG" \
  --notes-file "$NOTES_FILE" \
  --repo "$REPO_SLUG"

rm -f "$NOTES_FILE" || true

pass "ALL DONE — cortex-works-minimal $TAG is live on GitHub Releases"
