const { chromium } = require("playwright");
const { createServer } = require("node:http");
const { readFileSync } = require("node:fs");
const path = require("node:path");
const assert = require("node:assert/strict");

(async () => {
  const nonce = "synthetic-study-player";
  const html = readFileSync(path.join(__dirname, "../src-tauri/resources/study-player.html"), "utf8")
    .replaceAll("__SCRIPT_NONCE__", nonce);
  const server = createServer((request, response) => {
    if (request.url === "/host") {
      response.writeHead(200, { "Content-Type": "text/html" });
      response.end("<!doctype html><html><body></body></html>");
      return;
    }
    response.writeHead(200, {
      "Content-Type": "text/html",
      "Referrer-Policy": "strict-origin-when-cross-origin",
      "Content-Security-Policy": `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'; frame-src https://www.youtube-nocookie.com; base-uri 'none'; form-action 'none'; object-src 'none'`,
    });
    response.end(html);
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    let embedRequest;
    await page.route("https://www.youtube-nocookie.com/**", async route => {
      embedRequest = route.request();
      await route.fulfill({ contentType: "text/html", body: `<script>
        addEventListener('message', event => {
          const data = JSON.parse(event.data);
          const reply = message => parent.postMessage(JSON.stringify(message), event.origin);
          if (data.event === 'listening') reply({event:'onReady',info:null});
          if (data.func === 'seekTo') reply({event:'infoDelivery',info:{currentTime:data.args[0],duration:200,playerState:2,privateField:'must not cross wrapper'}});
          if (data.func === 'playVideo') reply({event:'onStateChange',info:1});
          if (data.func === 'loadVideoById') reply({event:'onError',info:'unapproved command'});
        });
      </script>` });
    });
    await page.goto(`${origin}/host`);
    await page.evaluate(({ origin }) => {
      window.received = [];
      const frame = document.createElement("iframe");
      frame.id = "study";
      frame.sandbox = "allow-scripts allow-same-origin";
      frame.src = `${origin}/token/player#video=aqz-KE-bpKQ&start=42`;
      document.body.append(frame);
      addEventListener("message", event => {
        if (event.source === frame.contentWindow && event.origin === origin) window.received.push(JSON.parse(event.data));
      });
    }, { origin });
    await page.waitForFunction(() => document.querySelector("iframe")?.contentWindow != null);
    await page.waitForTimeout(400);
    const send = data => page.evaluate(({ origin, data }) =>
      document.querySelector("iframe").contentWindow.postMessage(JSON.stringify(data), origin), { origin, data });
    await send({ event: "listening", id: "fixture", channel: "widget" });
    await page.waitForFunction(() => window.received.some(message => message.event === "onReady"));
    assert.equal(embedRequest.headers().referer, `${origin}/`, "YouTube receives a real HTTP Referer");
    const url = new URL(embedRequest.url());
    assert.equal(url.searchParams.get("start"), "42");
    assert.equal(url.searchParams.get("origin"), origin);
    assert.equal(url.searchParams.get("widget_referrer"), "https://com.grafium.app/");
    await send({ event: "command", func: "seekTo", args: [60, true] });
    await page.waitForFunction(() => window.received.some(message => message.info?.currentTime === 60));
    assert.equal(await page.evaluate(() => window.received.some(message => message.info?.privateField)), false);
    await send({ event: "command", func: "loadVideoById", args: ["other"] });
    await page.waitForTimeout(100);
    assert.equal(await page.evaluate(() => window.received.some(message => message.event === "onError")), false);
    assert.deepEqual(errors, []);
    console.log("PASS isolated study player: HTTP identity, timestamp resume, bounded bridge, and CSP");
  } finally {
    await browser.close();
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
