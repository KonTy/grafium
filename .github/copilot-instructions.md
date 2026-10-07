# Grafium contribution instructions

## Keep everyday UI concise

Keep controls, current results, errors, and actionable warnings visible, but put
explanatory prose behind on-demand help rather than repeating it in the main UI.
In Settings, reuse `SettingsHelp` for compact, labelled `?` buttons with
keyboard-accessible dialogs. Keep help searchable and preserve contextual F1.
Never hide destructive-action warnings or consent requirements in optional help.

## Keep contextual help current

Whenever functionality or user-facing behavior changes:

- Update the relevant contextual F1 help page in the tutorial/welcome graph.
- Update the main `Grafium Help` index when adding a new help topic.
- If the change introduces a new screen or meaningful context, add it to
  `ui/src/lib/help.ts` and wire its F1 context mapping in `ui/src/App.svelte`.
- When changing seeded tutorial content, increment `SEED_VERSION` in
  `ui/src-tauri/src/welcome.rs`. Seeding is fresh-install-only: a marker from
  any version we have ever shipped is a permanent opt-out, so an installed
  tutorial graph is never re-seeded, refreshed, or added to. Do not reintroduce
  an additive migration for existing graphs — it writes into a graph the user
  owns, and `welcome::tests::every_legacy_marker_opts_out_with_or_without_notes`
  will fail. The opt-out check ranges over `1..=SEED_VERSION`, so bumping the
  version needs no new marker check.
- Add or update a focused test for new help-context mappings or navigation.

Help documentation is part of the feature acceptance criteria. Do not finish a
user-facing functionality change while leaving its F1 guidance outdated.

## Bump the version for deployed builds

Before committing any user-facing build that will be deployed or distributed,
run `./scripts/bump-version.sh`. This bumps the patch version by default and
keeps the Rust workspace, Tauri application, npm package, and lockfiles in sync.

Bump exactly once per deployable change set. Do not bump for intermediate
compiles, tests, or rebuilds of the same commit.

## Deploy completed changes for local testing

After completing and validating user-facing changes, deploy them locally by
default so the user can test without a separate deployment request, unless
the user explicitly asks not to deploy.

There is exactly one desktop install, and it is the per-user one. Build the
frontend before the native release, then use `./scripts/deploy-local.sh` and
verify the installed `grafium --version`.

Do not also install a system-wide copy. Two installs meant `grafium` could
resolve to either one and neither the version string nor the start menu said
which, and the system path additionally overwrote files owned by the
`grafium-bin` package. `scripts/install-system.py` is kept only for packaging
a release for other machines; it is not part of deploying for local testing,
and it must not be run here.

Grafium has exactly one identity, end to end: the installed executable, the
window class it reports, the `~/.local/bin/grafium` launcher and the single
`~/.local/share/applications/grafium.desktop` entry are all named `grafium`.
`install-desktop.sh` writes that one entry and removes the obsolete
`grafium-bin.desktop`. Do not add a second entry or a second executable name.
The menu indexer de-duplicates by name, so a duplicate does not show up as a
visible mistake — it silently decides which build the menu launches. Keep
`Categories=` to a single main category too, or some menus list Grafium once
per category.

Do not interrupt running instances or modify personal graphs for smoke tests;
tell the user to fully quit and reopen Grafium. Local deployment does not
authorize a remote push or release publication.

Keep nothing we can rebuild: disk space has repeatedly run out because of old
binaries and backups. `deploy-local.sh` keeps its verified backup only until
the switch succeeds, then deletes older installed builds and those backups. A
build a running Grafium still uses survives until the next deploy.
`deploy-local.sh` then clears the checkout's Cargo build cache, so the next
build starts fresh. Do not keep release binaries, installers or build caches
after a deploy unless the user asks (`GRAFIUM_KEEP_BUILD_CACHE=1` keeps the
cache). There are no binary backups to roll back to: if a deployed build is
broken, fix the bug and redeploy.

A cold build can leave `target/release/bundled-libs/` with only part of the
llama.cpp/GGML libraries, because `ui/src-tauri/build.rs` may run while that
CMake build is still emitting `.so`s. The binary then fails to start with a
missing `libggml.so.0`, and `deploy-local.sh` stops with `cp: cannot stat
.../libggml.so`. This is self-correcting: run the same `cargo build --release`
again and the build script re-runs and finishes the copy. Do not delete
`target/` to work around it — a fresh build just loses the cache and hits the
same race. Commit before building, too: the version string embeds the commit
and is marked `dirty` for an uncommitted tree, so a build made first reports
the wrong revision.

## Never lose user data

Data is the opposite of binaries: never lose any of it. That covers graph
folders (pages, journals, assets), graph databases and indexes, private
Library data (`reader.json` locations, progress and bookmarks, and the Library
index), app settings such as layout and keyboard shortcuts, and job history.

- Change stored formats only through in-place migrations that keep existing
  files readable. Write the new format only after reading the old one
  successfully.
- Never delete, reset or recreate user data to get past a bug.
- Smoke tests run against a throwaway `HOME` or graph, never the user's.
- Before anything that could change or remove the user's own data outside
  normal app use, keep a verified copy and confirm with the user.
