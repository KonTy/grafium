# Grafium contribution instructions

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

Build the frontend before the native release, then use
`./scripts/deploy-local.sh` and verify the installed `grafium --version`.
Preserve the existing installation through the script's verified backups.
Do not interrupt running instances or modify personal graphs for smoke tests;
tell the user to fully quit and reopen Grafium. Local deployment does not
authorize a remote push or release publication.
