const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { openEditor } = require("./keyboardSelection.ui.cjs");

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const { page, errors } = await openEditor(browser, {
      beforeNavigate: page => page.addInitScript(() => {
        if (window !== window.top) return;
        const conflict = {
          target_id: "fixture-usb", target_name: "Fixture USB",
          rel_path: "assets/image.png", backup_path: ".grafium/sync-recovery/image-remote.png", recorded_at: 1,
        };
        window.__syncFixture = { conflicts: [conflict], writes: [], stale: false };
        function install(internals) {
          const original = internals.invoke;
          internals.invoke = async (command, args = {}) => {
            const fixture = window.__syncFixture;
            if (command === "sync_list_conflicts") return structuredClone(fixture.conflicts);
            if (command === "sync_get_conflict_state") return {
              graph_path: "/synthetic/keyboard-selection", rel_path: conflict.rel_path,
              local_hash: null, remote_hash: "original-binary-hash", local_size: null, remote_size: 0,
            };
            if (command === "sync_resolve_conflict") {
              fixture.writes.push(structuredClone(args));
              if (fixture.stale) throw new Error("The remote revision changed");
              fixture.conflicts = [];
              return { pushed: [], pulled: [], conflicts: [], deleted_local: [], deleted_remote: [conflict.rel_path], errors: [] };
            }
            return original(command, args);
          };
          return internals;
        }
        let internals = window.__TAURI_INTERNALS__;
        if (internals) internals = install(internals);
        Object.defineProperty(window, "__TAURI_INTERNALS__", {
          configurable: true, get: () => internals, set: value => { internals = install(value); },
        });
      }),
    });
    await page.evaluate(() => window.dispatchEvent(new CustomEvent("toggle-reference-panel")));
    const panel = page.getByRole("complementary", { name: "Reading panel" });
    await panel.getByRole("tab", { name: /Conflicts/ }).click();
    await panel.getByRole("button", { name: "Resolve assets/image.png", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Resolve sync conflict", exact: true });
    await dialog.waitFor();
    assert.equal(await dialog.getByRole("button", { name: "Confirm choice" }).isDisabled(), true);
    await dialog.getByLabel("Local: deleted", { exact: true }).check();
    await dialog.getByText(/Confirm deletion of the remote file/).waitFor();
    assert.deepEqual(await page.evaluate(() => window.__syncFixture.writes), []);
    await dialog.getByRole("button", { name: "Confirm choice" }).click();
    await dialog.waitFor({ state: "hidden" });
    assert.deepEqual(await page.evaluate(() => window.__syncFixture.writes), [{
      graphPath: "/synthetic/keyboard-selection", targetId: "fixture-usb",
      relPath: "assets/image.png", expectedLocalHash: null,
      expectedRemoteHash: "original-binary-hash", chosen: "local",
    }]);
    assert.equal(await page.evaluate(() => window.__selectionState.calls
      .filter(call => call.cmd === "create_page" && call.args.title?.includes("image.png")).length), 0);
    assert.deepEqual(errors, []);
    console.log("Sync conflict UI: binary resolution, explicit deletion confirmation, and revision-pinned IPC passed");
    await page.close();
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
