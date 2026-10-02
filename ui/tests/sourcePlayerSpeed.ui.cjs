const { chromium } = require("playwright");
const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");

const origin = (process.env.UI_TEST_URL || process.env.GRAFIUM_TEST_URL || "http://127.0.0.1:5218").replace(/\/$/, "");

function silentAudio() {
  const samples = 8000 * 30;
  const bytes = Buffer.alloc(44 + samples * 2);
  bytes.write("RIFF", 0); bytes.writeUInt32LE(bytes.length - 8, 4);
  bytes.write("WAVEfmt ", 8); bytes.writeUInt32LE(16, 16);
  bytes.writeUInt16LE(1, 20); bytes.writeUInt16LE(1, 22);
  bytes.writeUInt32LE(8000, 24); bytes.writeUInt32LE(16000, 28);
  bytes.writeUInt16LE(2, 32); bytes.writeUInt16LE(16, 34);
  bytes.write("data", 36); bytes.writeUInt32LE(samples * 2, 40);
  return bytes;
}

(async () => {
  // Both sources are generated in memory; no graph, private media, or temp files.
  const audio = silentAudio();
  const video = execFileSync("ffmpeg", ["-hide_banner", "-loglevel", "error",
    "-filter_threads", "1", "-f", "lavfi", "-i", "color=c=blue:s=160x90:r=5:d=30",
    "-threads", "1", "-c:v", "libvpx", "-f", "webm", "pipe:1"]);
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.route(`${origin}/source-player-speed-fixture`, route =>
      route.fulfill({ contentType: "text/html", body: "<!doctype html><html><body></body></html>" }));
    for (const [name, bytes, contentType] of [["audio.wav", audio, "audio/wav"], ["video.webm", video, "video/webm"]]) {
      await page.route(`${origin}/synthetic-${name}`, route => {
        const range = route.request().headers().range?.match(/^bytes=(\d+)-(\d*)$/);
        const start = range ? Number(range[1]) : 0;
        const end = range?.[2] ? Math.min(Number(range[2]), bytes.length - 1) : bytes.length - 1;
        return route.fulfill({
          status: range ? 206 : 200, contentType, body: bytes.subarray(start, end + 1),
          headers: { "Accept-Ranges": "bytes",
            ...(range ? { "Content-Range": `bytes ${start}-${end}/${bytes.length}` } : {}) },
        });
      });
    }
    async function initialize() {
      await page.goto(`${origin}/source-player-speed-fixture`);
      await page.evaluate(async () => {
        window.speedFixture = await import("/tests/fixtures/sourcePlayerSpeed.ts");
        window.speedFixture.preferences.loadReaderPlaybackPreferences();
      });
    }
    async function mountSource(kind, local = false) {
      await page.evaluate(args => window.speedFixture.mountSource(args), { kind, local, origin });
      await page.waitForFunction(() => {
        const media = document.querySelector("audio,video");
        return media?.readyState >= 2 && Math.abs(media.currentTime - 3) < .1;
      });
    }
    await initialize();
    for (const kind of ["audio", "video"]) {
      for (const local of [false, true]) {
        await mountSource(kind, local);
        const media = page.locator(kind);
        const controls = page.getByRole("region", { name: "Media playback controls" });
        const speed = controls.getByRole("combobox", { name: "Speed", exact: true });
        assert.equal(await media.evaluate(element => element.paused), true, "opening a source never autoplays");
        await speed.selectOption("4");
        assert.deepEqual(await media.evaluate(element => ({
          rate: element.playbackRate, pitch: element.preservesPitch, time: element.currentTime, paused: element.paused,
        })), { rate: 4, pitch: true, time: 3, paused: true });
        await controls.getByRole("button", { name: "Resume", exact: true }).click();
        await page.waitForFunction(() => document.querySelector("audio,video").currentTime > 4);
        assert.equal(await media.evaluate(element => element.playbackRate), 4, `${kind} uses actual native 4× while playing`);
        await speed.selectOption("3.5");
        assert.equal(await media.evaluate(element => element.playbackRate === 3.5 && !element.paused), true, "live speed changes do not pause playback");
        await speed.selectOption("4");
        await controls.getByRole("button", { name: "Pause", exact: true }).click();
        const pausedTime = await media.evaluate(element => element.currentTime);
        await speed.selectOption("2.5");
        assert.equal(await media.evaluate(element => element.currentTime), pausedTime, "speed changes preserve position");
        assert.equal(await media.evaluate(element => element.paused), true);
        await page.evaluate(() => window.speedFixture.preferences.setMediaPlaybackRate(4));
        await page.waitForFunction(() => document.querySelector("audio,video").playbackRate === 4);
        await controls.getByRole("button", { name: "Stop", exact: true }).click();
        assert.equal(await media.evaluate(element => element.currentTime), pausedTime, "stop retains the bookmark position");
        await media.evaluate(element => { window.previousSpeedMedia = element; });
        await mountSource(kind, local);
        assert.equal(await page.evaluate(() => window.previousSpeedMedia.paused && !window.previousSpeedMedia.getAttribute("src")), true);
        assert.equal(await page.locator(kind).evaluate(element => element.playbackRate), 4, "speed survives source navigation");
      }
    }
    await initialize();
    await mountSource("video");
    assert.equal(await page.locator("video").evaluate(element => element.playbackRate), 4, "speed survives full reload");
    assert.equal(await page.getByRole("combobox", { name: "Speed", exact: true }).inputValue(), "4");
    await page.evaluate(() => {
      const media = document.querySelector("video");
      let actual = media.playbackRate;
      Object.defineProperty(media, "playbackRate", {
        configurable: true, get: () => actual,
        set: value => {
          if (value > 2) throw new DOMException("Synthetic unsupported speed", "NotSupportedError");
          actual = value;
        },
      });
    });
    const speed = page.getByRole("combobox", { name: "Speed", exact: true });
    await speed.selectOption("1");
    await speed.selectOption("4");
    await page.getByRole("alert").filter({ hasText: "Could not change playback speed" }).waitFor();
    assert.equal(await speed.inputValue(), "1", "unsupported speed is not falsely displayed as 4×");
    assert.equal(await page.locator("video").evaluate(element => element.currentTime), 3);
    await speed.selectOption("2");
    assert.equal(await page.getByRole("alert").count(), 0, "supported selection clears the error");
    assert.deepEqual(errors, []);
    console.log("PASS foreground source speeds: actual 4× audio/video, pitch, no autoplay, position, navigation/reload, and unsupported-rate feedback");
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
