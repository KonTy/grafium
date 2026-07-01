#!/usr/bin/env bash
set -e

# Build script: runs tests, bumps patch version, then builds.
# Usage: ./build.sh

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "══════════════════════════════════════════════"
echo "  Grafium - Build Pipeline"
echo "══════════════════════════════════════════════"

# ─── 1. Run Rust tests ────────────────────────────
echo ""
echo "▶ Running Rust tests..."
cargo test --workspace 2>&1
if [ $? -ne 0 ]; then
    echo "✗ Tests failed. Build aborted."
    exit 1
fi
echo "✓ All Rust tests passed."

# ─── 2. Bump version (patch/build) ───────────────
echo ""
echo "▶ Bumping patch/build version..."

# Read current version from workspace Cargo.toml
CURRENT_VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')

# Parse major.minor.patch and increment patch like Windows build number
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"
NEW_PATCH=$((PATCH + 1))
NEW_VERSION="$MAJOR.$MINOR.$NEW_PATCH"

echo "  $CURRENT_VERSION → $NEW_VERSION"

# Update all version references
sed -i "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
sed -i "0,/^version = \"$CURRENT_VERSION\"/s//version = \"$NEW_VERSION\"/" ui/src-tauri/Cargo.toml
sed -i "s/\"version\": \"$CURRENT_VERSION\"/\"version\": \"$NEW_VERSION\"/" ui/src-tauri/tauri.conf.json
sed -i "s/\"version\": \"$CURRENT_VERSION\"/\"version\": \"$NEW_VERSION\"/" ui/package.json

echo "✓ Version bumped to $NEW_VERSION"

# ─── 3. Build frontend ───────────────────────────
echo ""
echo "▶ Building frontend..."
cd ui
npm run build 2>&1
cd ..
echo "✓ Frontend build complete."

# ─── 4. Build Rust backend (embeds frontend dist) ─
echo ""
echo "▶ Building Rust backend..."
cargo build --release 2>&1
echo "✓ Rust build complete."

# ─── 5. Done ─────────────────────────────────────
echo ""
echo "══════════════════════════════════════════════"
echo "  ✓ Build successful - v$NEW_VERSION"
echo "  Binary: target/release/grafium"
echo "══════════════════════════════════════════════"
