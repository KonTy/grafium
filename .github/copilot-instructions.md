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
