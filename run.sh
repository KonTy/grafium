#!/bin/sh
# Launch script for the PKM app
# Disables WebKitGTK's DMA-BUF renderer to avoid grey screen on some GPUs
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
exec "$SCRIPT_DIR/target/debug/logseq-clone" "$@"
