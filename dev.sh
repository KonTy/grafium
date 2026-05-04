#!/bin/bash
# Grafium Dev Launch Script
# Handles all the Linux/WebKitGTK gotchas automatically:
# 1. Kills any existing instances
# 2. Rebuilds dist/ (required by Tauri's generate_context! macro)
# 3. Launches tauri dev with proper GTK/WebKit env vars
# 4. incognito: true in tauri.conf.json prevents WebKitGTK disk caching

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
UI_DIR="$SCRIPT_DIR/ui"

echo "==> Killing existing instances..."
pkill -9 -f "target/debug/grafium" 2>/dev/null || true
pkill -9 -f "vite" 2>/dev/null || true
lsof -ti:5173 2>/dev/null | xargs kill -9 2>/dev/null || true
sleep 0.5

echo "==> Building frontend dist (required for Tauri compile)..."
cd "$UI_DIR"
npx vite build 2>&1 | tail -3

echo "==> Launching tauri dev..."
export GDK_BACKEND=x11
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export WEBKIT_DISABLE_COMPOSITING_MODE=1

exec npm run tauri -- dev
