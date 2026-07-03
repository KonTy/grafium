#!/usr/bin/env python3
"""Generate a massive Grafium test graph for performance testing.

Writes pages, blocks, cross-reference links (including reciprocal "double
links") and the FTS index directly into a fresh SQLite `index.db`, which is the
query source the app reads. This is far faster than generating millions of
markdown files and letting the app reindex them.

The generated graph is a normal Grafium graph directory:

    <out>/
      pages/          (empty — block content lives in the DB)
      journals/       (empty)
      .grafium/
        index.db      (populated here)

By default it targets ~1,000,000 pages and ~20,000,000 blocks with dense
cross-references. Everything is configurable — start smaller to validate:

    python3 scripts/generate_perf_graph.py --pages 5000
    python3 scripts/generate_perf_graph.py            # full 1M run
    python3 scripts/generate_perf_graph.py --register # also switch the app to it

The app must be CLOSED while this runs (SQLite is locked by the running app).
"""

import argparse
import os
import json
import random
import shutil
import sqlite3
import sys
import time
from datetime import date

# ---- Schema (mirrors core/src/db/schema.rs — only the tables we populate) ----
SCHEMA = """
CREATE TABLE IF NOT EXISTS pages (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL UNIQUE,
    file_path TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    is_journal INTEGER NOT NULL DEFAULT 0,
    properties TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS blocks (
    id TEXT PRIMARY KEY,
    page_id TEXT NOT NULL,
    parent_id TEXT,
    order_index INTEGER NOT NULL DEFAULT 0,
    content TEXT NOT NULL DEFAULT '',
    block_type TEXT NOT NULL DEFAULT 'text',
    properties TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (page_id) REFERENCES pages(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS links (
    from_block_id TEXT NOT NULL,
    to_page_id TEXT NOT NULL,
    link_type TEXT NOT NULL DEFAULT 'page',
    PRIMARY KEY (from_block_id, to_page_id, link_type),
    FOREIGN KEY (from_block_id) REFERENCES blocks(id) ON DELETE CASCADE
);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_blocks USING fts5(
    block_id UNINDEXED,
    content,
    tokenize='porter unicode61'
);
"""

INDEXES = """
CREATE INDEX IF NOT EXISTS idx_pages_title ON pages(title);
CREATE INDEX IF NOT EXISTS idx_pages_title_lower ON pages(lower(title));
CREATE INDEX IF NOT EXISTS idx_pages_journal_title ON pages(title DESC) WHERE is_journal = 1;
CREATE INDEX IF NOT EXISTS idx_pages_updated ON pages(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_blocks_page ON blocks(page_id, order_index);
CREATE INDEX IF NOT EXISTS idx_blocks_parent ON blocks(parent_id, order_index);
CREATE INDEX IF NOT EXISTS idx_blocks_type ON blocks(block_type) WHERE block_type != 'text';
CREATE INDEX IF NOT EXISTS idx_blocks_updated ON blocks(updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_links_to ON links(to_page_id, link_type);
CREATE INDEX IF NOT EXISTS idx_links_from ON links(from_block_id);
"""

WORDS = (
    "system design distributed cache latency throughput index query planner vector "
    "embedding neuron gradient tensor kernel pipeline schema migration rollback commit "
    "atlas orbit vector matrix signal entropy protocol handshake payload manifest "
    "graph node edge traversal backlink reference citation summary insight hypothesis "
    "experiment benchmark profile flamegraph allocation heap stack frame closure async "
    "await mutex channel actor message queue durable idempotent consensus quorum shard"
).split()


def uid(n: int, salt: int = 0) -> str:
    """Deterministic uuid-shaped id from an integer (fast, no uuid4 overhead)."""
    v = (n * 0x9E3779B97F4A7C15 + salt * 0xC2B2AE3D27D4EB4F) & ((1 << 128) - 1)
    h = f"{v:032x}"
    return f"{h[0:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:32]}"


def page_title(i: int) -> str:
    return f"Page {i:07d}"


def journal_titles(n: int):
    """Yield n unique, valid, descending journal date strings (YYYY-MM-DD).

    The most recent entry is today; earlier entries walk back one day at a time.
    For very large n the window is shifted so every date stays within the range
    the app (and Python's `date`) supports, so millions of unique journals are
    possible.
    """
    min_ord = date.min.toordinal()
    max_ord = date.max.toordinal()
    span = max_ord - min_ord + 1
    n = max(0, min(n, span))
    start = date.today().toordinal()
    end = start - n + 1
    if end < min_ord:
        start += (min_ord - end)
        end = min_ord
    if start > max_ord:
        start = max_ord
        end = start - n + 1
    for o in range(start, end - 1, -1):
        yield date.fromordinal(o).isoformat()


def make_paragraph(rng: random.Random, n: int) -> str:
    return " ".join(rng.choice(WORDS) for _ in range(n)).capitalize()


def human_bytes(n: float) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024:
            return f"{n:.1f}{unit}"
        n /= 1024
    return f"{n:.1f}PB"


def main() -> int:
    ap = argparse.ArgumentParser(description="Generate a huge Grafium test graph.")
    ap.add_argument("--pages", type=int, default=1_000_000, help="number of pages")
    ap.add_argument("--journals", type=int, default=0,
                    help="number of dated journal entries (is_journal=1) to also create")
    ap.add_argument("--append", action="store_true",
                    help="add to an EXISTING graph at --out (skip page generation, keep data); "
                         "use with --journals to backfill journals without rebuilding pages")
    ap.add_argument("--min-blocks", type=int, default=8, help="min blocks per page")
    ap.add_argument("--max-blocks", type=int, default=32, help="max blocks per page")
    ap.add_argument("--links-per-page", type=int, default=5,
                    help="avg outgoing wikilinks per page (drives backlinks)")
    ap.add_argument("--reciprocal", type=float, default=0.35,
                    help="fraction of links that are made reciprocal (double links)")
    ap.add_argument("--out", default=os.path.expanduser(
        "~/.local/share/com.grafium.app/perf-test-graph"),
        help="graph directory to create")
    ap.add_argument("--no-fts", action="store_true", help="skip full-text index (smaller/faster)")
    ap.add_argument("--force", action="store_true", help="overwrite existing --out directory")
    ap.add_argument("--register", action="store_true",
                    help="add to the app's graphs.json and make it the current graph")
    ap.add_argument("--seed", type=int, default=42)
    ap.add_argument("--batch-pages", type=int, default=2000,
                    help="pages per DB transaction flush")
    args = ap.parse_args()

    rng = random.Random(args.seed)
    out = os.path.abspath(args.out)
    pages_dir = os.path.join(out, "pages")
    journals_dir = os.path.join(out, "journals")
    meta_dir = os.path.join(out, ".grafium")
    db_path = os.path.join(meta_dir, "index.db")

    avg_blocks = (args.min_blocks + args.max_blocks) / 2
    est_blocks = int(args.pages * avg_blocks)
    est_links = int(args.pages * args.links_per_page * (1 + args.reciprocal))
    if args.append:
        print(f"APPEND mode: adding {args.journals:,} journal entries to existing graph")
    else:
        print(f"Target: {args.pages:,} pages  ~{est_blocks:,} blocks  ~{est_links:,} links"
              f"  FTS={'off' if args.no_fts else 'on'}")
    print(f"Output graph: {out}")

    if args.append:
        if not os.path.exists(db_path):
            print(f"ERROR: --append needs an existing DB at {db_path}", file=sys.stderr)
            return 1
        if args.journals <= 0:
            print("ERROR: --append requires --journals N (nothing to add).", file=sys.stderr)
            return 1
        os.makedirs(journals_dir, exist_ok=True)
    else:
        if os.path.exists(out):
            if not args.force:
                print(f"ERROR: {out} already exists. Use --force to overwrite.", file=sys.stderr)
                return 1
            shutil.rmtree(out)

        os.makedirs(pages_dir, exist_ok=True)
        os.makedirs(journals_dir, exist_ok=True)
        os.makedirs(meta_dir, exist_ok=True)

        # Remove any stale WAL/SHM sidecars.
        for ext in ("-wal", "-shm"):
            p = db_path + ext
            if os.path.exists(p):
                os.remove(p)

    conn = sqlite3.connect(db_path)
    cur = conn.cursor()
    # Bulk-load PRAGMAs: durability off for speed, restored to WAL at the end.
    cur.executescript("""
        PRAGMA journal_mode=OFF;
        PRAGMA synchronous=OFF;
        PRAGMA temp_store=MEMORY;
        PRAGMA cache_size=-262144;   -- ~256MB page cache
        PRAGMA foreign_keys=OFF;
    """)
    cur.executescript(SCHEMA)

    now_ms = int(time.time() * 1000)
    # Number of regular pages available as link targets. In append mode we read
    # it from the existing DB instead of generating pages.
    if args.append:
        P = cur.execute("SELECT count(*) FROM pages WHERE is_journal = 0").fetchone()[0]
        P = max(P, 1)
    else:
        P = args.pages
    start = time.time()

    page_rows = []
    block_rows = []
    link_rows = []
    fts_rows = []
    pending_reciprocal = {}  # target_page_idx -> list of source_page_idx that linked to it

    def flush():
        cur.executemany(
            "INSERT OR IGNORE INTO pages(id,title,file_path,created_at,updated_at,is_journal,properties)"
            " VALUES(?,?,?,?,?,?,'{}')", page_rows)
        cur.executemany(
            "INSERT OR IGNORE INTO blocks(id,page_id,parent_id,order_index,content,block_type,properties,created_at,updated_at)"
            " VALUES(?,?,?,?,?,'text','{}',?,?)", block_rows)
        if link_rows:
            cur.executemany(
                "INSERT OR IGNORE INTO links(from_block_id,to_page_id,link_type) VALUES(?,?,'page')",
                link_rows)
        if fts_rows:
            cur.executemany("INSERT INTO fts_blocks(block_id,content) VALUES(?,?)", fts_rows)
        conn.commit()
        page_rows.clear(); block_rows.clear(); link_rows.clear(); fts_rows.clear()

    page_loop = range(1, P + 1) if not args.append else range(0)
    for i in page_loop:
        pid = uid(i, salt=1)
        title = page_title(i)
        # file_path points into pages/ but we intentionally do NOT create the file
        # (avoids a million tiny files); the app recreates it lazily on first edit.
        page_rows.append((pid, title, os.path.join("pages", title + ".md"), now_ms, now_ms, 0))

        nblocks = rng.randint(args.min_blocks, args.max_blocks)
        # Decide this page's outgoing link targets (random other pages).
        nlinks = max(0, int(rng.gauss(args.links_per_page, 1.5)))
        targets = [rng.randint(1, P) for _ in range(nlinks) if _ or True]
        targets = [t for t in targets if t != i]

        first_block_id = None
        for b in range(nblocks):
            bid = uid(i * 64 + b, salt=2)
            if b == 0:
                first_block_id = bid
                content = f"# {title}"
            elif targets and b <= len(targets):
                tgt = targets[b - 1]
                content = (f"{make_paragraph(rng, rng.randint(4, 10))} "
                           f"[[{page_title(tgt)}]] {make_paragraph(rng, rng.randint(2, 6))}")
                link_rows.append((bid, uid(tgt, salt=1)))
                if rng.random() < args.reciprocal:
                    pending_reciprocal.setdefault(tgt, []).append(i)
            else:
                content = "- " + make_paragraph(rng, rng.randint(6, 18))
            block_rows.append((bid, pid, None, b, content, now_ms, now_ms))
            if not args.no_fts:
                fts_rows.append((bid, content))

        # Emit reciprocal (double) links back to THIS page from pages that
        # earlier picked us as a reciprocal target and have already been created.
        for ri, src in enumerate(pending_reciprocal.pop(i, [])):
            # Add a link block on the current page pointing back at the source.
            bid = uid(i * 1024 + ri, salt=3)
            src_title = page_title(src)
            content = f"Related back-reference: [[{src_title}]]"
            block_rows.append((bid, pid, None, nblocks, content, now_ms, now_ms))
            link_rows.append((bid, uid(src, salt=1)))
            if not args.no_fts:
                fts_rows.append((bid, content))

        if i % args.batch_pages == 0:
            flush()
            elapsed = time.time() - start
            rate = i / elapsed
            eta = (P - i) / rate
            sys.stdout.write(
                f"\r  {i:,}/{P:,} pages  {rate:,.0f} pg/s  ETA {eta/60:5.1f} min  "
                f"db {human_bytes(os.path.getsize(db_path))}   ")
            sys.stdout.flush()

    flush()

    # ---- Journal entries (is_journal=1, dated titles) ----
    if args.journals > 0:
        J = args.journals
        jstart = time.time()
        for j, jtitle in enumerate(journal_titles(J), start=1):
            jpid = uid(j, salt=5)
            page_rows.append(
                (jpid, jtitle, os.path.join("journals", jtitle + ".md"), now_ms, now_ms, 1))

            nblocks = rng.randint(args.min_blocks, args.max_blocks)
            nlinks = max(0, int(rng.gauss(args.links_per_page, 1.5)))
            # Journals cross-link into the regular page corpus (creates backlinks there).
            targets = [rng.randint(1, P) for _ in range(nlinks)]

            for b in range(nblocks):
                bid = uid(j * 64 + b, salt=4)
                if b == 0:
                    content = f"# {jtitle}"
                elif targets and b <= len(targets):
                    tgt = targets[b - 1]
                    content = (f"{make_paragraph(rng, rng.randint(4, 10))} "
                               f"[[{page_title(tgt)}]] {make_paragraph(rng, rng.randint(2, 6))}")
                    link_rows.append((bid, uid(tgt, salt=1)))
                else:
                    content = "- " + make_paragraph(rng, rng.randint(6, 18))
                block_rows.append((bid, jpid, None, b, content, now_ms, now_ms))
                if not args.no_fts:
                    fts_rows.append((bid, content))

            if j % args.batch_pages == 0:
                flush()
                elapsed = time.time() - jstart
                rate = j / elapsed
                eta = (J - j) / rate
                sys.stdout.write(
                    f"\r  {j:,}/{J:,} journals  {rate:,.0f} jr/s  ETA {eta/60:5.1f} min  "
                    f"db {human_bytes(os.path.getsize(db_path))}   ")
                sys.stdout.flush()
        flush()
        print()

    print("\nBuilding indexes...")
    cur.executescript(INDEXES)
    # Drop the legacy is_journal-only index if a pre-existing DB still has it;
    # idx_pages_journal_title supersedes it and keeps journal listing sort-free.
    cur.execute("DROP INDEX IF EXISTS idx_pages_journal")

    print("Optimizing (WAL + analyze)...")
    cur.execute("PRAGMA journal_mode=WAL")
    cur.execute("ANALYZE")
    conn.commit()
    conn.close()

    total = time.time() - start
    # Reopen quickly for a count summary.
    c2 = sqlite3.connect(db_path)
    npages_total = c2.execute("SELECT count(*) FROM pages WHERE is_journal=0").fetchone()[0]
    njournals_total = c2.execute("SELECT count(*) FROM pages WHERE is_journal=1").fetchone()[0]
    nblocks_total = c2.execute("SELECT count(*) FROM blocks").fetchone()[0]
    nlinks_total = c2.execute("SELECT count(*) FROM links").fetchone()[0]
    c2.close()
    print(f"Done in {total/60:.1f} min: {npages_total:,} pages, {njournals_total:,} journals, "
          f"{nblocks_total:,} blocks, {nlinks_total:,} links. DB {human_bytes(os.path.getsize(db_path))}")

    if args.register:
        register_graph(out)
    else:
        print("\nTo use it: open the app, switch graph to:")
        print(f"  {out}")
        print("Or re-run with --register to switch automatically.")
    return 0


def register_graph(out: str) -> None:
    cfg_path = os.path.expanduser("~/.local/share/com.grafium.app/graphs.json")
    try:
        with open(cfg_path) as f:
            cfg = json.load(f)
    except Exception:
        cfg = {"graphs": [], "current": None}
    # Back up before modifying.
    if os.path.exists(cfg_path):
        shutil.copy2(cfg_path, cfg_path + ".bak")
    name = "Perf Test Graph"
    graphs = cfg.get("graphs", [])
    if not any(g.get("path") == out for g in graphs):
        graphs.append({"name": name, "path": out})
    cfg["graphs"] = graphs
    cfg["current"] = out
    with open(cfg_path, "w") as f:
        json.dump(cfg, f, indent=2)
    print(f"\nRegistered and switched current graph to: {out}")
    print(f"(backup saved to {cfg_path}.bak) — restart the app to load it.")


if __name__ == "__main__":
    raise SystemExit(main())
