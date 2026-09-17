# Grafium contribution instructions

## Keep contextual help current

Whenever functionality or user-facing behavior changes:

- Update the relevant contextual F1 help page in the tutorial/welcome graph.
- Update the main `Grafium Help` index when adding a new help topic.
- If the change introduces a new screen or meaningful context, add it to
  `ui/src/lib/help.ts` and wire its F1 context mapping in `ui/src/App.svelte`.
- When changing seeded tutorial content, increment the tutorial seed marker in
  `ui/src-tauri/src/lib.rs` and preserve the existing migration behavior for
  installed tutorial graphs.
- Add or update a focused test for new help-context mappings or navigation.

Help documentation is part of the feature acceptance criteria. Do not finish a
user-facing functionality change while leaving its F1 guidance outdated.
