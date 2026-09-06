// Runs the browser UI checks against a dev server it starts and stops itself.
//
// Kept separate from `npm test` because it needs a browser and a server, which
// is a slower and heavier thing to ask for than the unit tests. Run it before
// shipping UI changes: it covers the failures that only appear once components
// run together, which `vitest` and `svelte-check` both pass straight through.
const { spawn } = require("node:child_process");
const path = require("node:path");

const PORT = Number(process.env.UI_TEST_PORT ?? 5199);
const URL = `http://localhost:${PORT}/`;
const uiDir = path.resolve(__dirname, "..");

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitForServer(timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(URL);
      if (res.ok) return;
    } catch {
      // Not listening yet.
    }
    await wait(300);
  }
  throw new Error(`dev server did not come up on ${URL} within ${timeoutMs}ms`);
}

const run = (cmd, args, opts) =>
  new Promise((resolve, reject) => {
    const child = spawn(cmd, args, { stdio: "inherit", ...opts });
    child.on("error", reject);
    child.on("exit", (code) => resolve(code ?? 1));
  });

(async () => {
  const server = spawn(
    "npx",
    ["vite", "--port", String(PORT), "--strictPort"],
    { cwd: uiDir, stdio: "ignore", detached: true },
  );
  // Killed via the process group: vite spawns children of its own, and killing
  // only the parent leaves the port held and the next run failing on
  // --strictPort.
  const stopServer = () => {
    try {
      process.kill(-server.pid, "SIGTERM");
    } catch {
      // Already gone.
    }
  };
  process.on("exit", stopServer);
  process.on("SIGINT", () => {
    stopServer();
    process.exit(130);
  });

  try {
    await waitForServer();
    const code = await run("node", [path.join(__dirname, "allPages.ui.cjs")], {
      cwd: uiDir,
      env: { ...process.env, UI_TEST_URL: URL },
    });
    process.exitCode = code;
  } catch (e) {
    console.error(e.message ?? e);
    process.exitCode = 1;
  } finally {
    stopServer();
  }
})();
