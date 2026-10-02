const { build } = require("esbuild");
const { readFile } = require("node:fs/promises");
const { spawn } = require("node:child_process");
const http = require("node:http");
const path = require("node:path");
const { epub, mobi, pdf } = require("./books.ui.cjs");

(async () => {
  const root = path.resolve(__dirname, "..");
  const runtime = await readFile(path.join(root, "public/book-reader/runtime.js"), "utf8");
  const security = await build({ absWorkingDir: root, entryPoints: ["src/lib/bookReaderSecurity.ts"],
    bundle: true, write: false, format: "iife", globalName: "BookSecurity" });
  const fixtures = [
    { format: "epub", bytes: [...epub()] },
    { format: "fb2", bytes: [...Buffer.from('<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0"><description><title-info><book-title>Native FB2</book-title><lang>en</lang></title-info></description><body><section><p>Select this original passage in FB2.</p></section></body></FictionBook>')] },
    { format: "mobi", bytes: [...mobi()] },
    { format: "pdf", bytes: [...pdf()] },
  ];
  const json = value => JSON.stringify(value).replace(/</g, "\\u003c");
  const html = `<!doctype html><html><body><script>${security.outputFiles[0].text}</script>
    <script>
    const runtime = ${json(runtime)};
    const fixtures = ${json(fixtures)};
    const errors = [];
    window.pwned = false;
    addEventListener('error', event => errors.push(event.message));
    addEventListener('unhandledrejection', event => errors.push(String(event.reason)));
    (async () => {
      const completed = [];
      for (const fixture of fixtures) {
        const token = crypto.randomUUID();
        const frame = document.createElement('iframe');
        frame.style.cssText = 'width:1100px;height:760px;border:0';
        frame.sandbox = BookSecurity.BOOK_FRAME_SANDBOX;
        const messages = [];
        let probe;
        const receive = event => {
          if (event.source === frame.contentWindow && event.data?.channel === 'reader-native-probe') probe = event.data;
          const message = BookSecurity.readReaderMessage(event, frame.contentWindow, token);
          if (message) messages.push(message);
        };
        addEventListener('message', receive);
        const send = (type, data = {}) => frame.contentWindow.postMessage({channel:'grafium-book', token, type, ...data}, '*');
        frame.onload = () => {
          send('bootstrap', {runtime});
          send('open', {format:fixture.format, bytes:Uint8Array.from(fixture.bytes).buffer, location:null});
        };
        frame.src = BookSecurity.readerFrameURL(token);
        document.body.append(frame);
        await new Promise((resolve, reject) => {
          let attempts = 0;
          const timer = setInterval(() => {
            const failure = messages.find(message => message.type === 'error');
            const selection = messages.find(message => message.type === 'selection');
            if (failure || ++attempts > 120) {
              clearInterval(timer);
              reject(new Error(fixture.format + ': ' + (failure?.message || 'reader/selection timed out')));
            } else if (selection && messages.some(message => message.type === 'location')) {
              clearInterval(timer);
              send('notes', {locations:[selection.location]});
              send('goto', {location:selection.location});
              send('size', {value:130});
              completed.push({format:fixture.format, quote:selection.quote, kind:selection.location.kind});
              resolve();
            }
          }, 200);
        });
        if (fixture.format !== 'pdf') {
          const selection = messages.find(message => message.type === 'selection');
          frame.style.width = '320px';
          send('size', {value:200}); send('bionic', {enabled:true}); send('flow', {value:'scrolled'});
          const checkAppearance = (enabled, scrolled, minWidth = 0, maxWidth = 320) => new Promise((resolve, reject) => {
            let attempts = 0;
            const timer = setInterval(() => {
              frame.contentWindow.postMessage({type:'native-fixture-probe', cfi:selection.location.cfi}, '*');
              const failure = messages.find(message => message.type === 'error');
              if (failure || ++attempts > 60) {
                clearInterval(timer);
                reject(new Error(fixture.format + ': native appearance/CFI failed: ' + JSON.stringify({probe, failure})));
              } else if (probe?.bold === enabled && probe?.scrolled === scrolled
                && probe.quote === selection.quote && probe.width > minWidth && probe.width <= maxWidth && probe.fontSize >= 24) {
                clearInterval(timer); resolve();
              }
            }, 100);
          });
          await checkAppearance(true, true);
          frame.style.width = '1100px';
          await checkAppearance(true, true, 880, 1100);
          send('bionic', {enabled:false}); send('flow', {value:'paginated'});
          await checkAppearance(false, false, 880, 1100);
          frame.style.width = '320px';
          await checkAppearance(false, false);
          completed.push({format:fixture.format + '-reflow', narrow:true, wide:true, bionic:true, canonicalQuote:true, layouts:2});
        }
        if (fixture.format === 'epub') {
          const narration = [];
          for (let section = 0; section < 2; section++) {
            let offset = 0;
            do {
              const requestId = 'native-narration-' + section + '-' + offset;
              send('read-aloud-segments', {requestId, section, offset});
              const batch = await new Promise((resolve, reject) => {
                let attempts = 0;
                const timer = setInterval(() => {
                  const result = messages.find(message =>
                    message.type === 'read-aloud-segments' && message.requestId === requestId);
                  const failure = messages.find(message => message.type === 'error');
                  if (failure || ++attempts > 100) {
                    clearInterval(timer);
                    reject(new Error(failure?.message || 'Native narration extraction timed out'));
                  } else if (result) { clearInterval(timer); resolve(result); }
                }, 50);
              });
              if (!batch.segments.length || batch.sectionCount !== 2 ||
                  (batch.nextOffset !== null && batch.nextOffset <= offset))
                throw new Error('Invalid native narration pagination');
              narration.push(...batch.segments);
              offset = batch.nextOffset;
            } while (offset !== null);
          }
          if (narration.length <= 145 ||
              !narration.some(segment => segment.text.includes('Offline paragraph 144.')) ||
              narration.some(segment => segment.text.includes('top.pwned')))
            throw new Error('Native narration omitted content or extracted executable text');
          const target = narration.find(segment => segment.text.includes('Return to the first passage.'));
          if (!target || target.locator.kind !== 'epub') throw new Error('Missing second-chapter narration');
          send('goto', {location:target.locator});
          await new Promise((resolve, reject) => {
            let attempts = 0;
            const timer = setInterval(() => {
              if (messages.some(message => message.type === 'location' && message.label.includes('Second chapter'))) {
                clearInterval(timer); resolve();
              } else if (++attempts > 100) {
                clearInterval(timer); reject(new Error('Native narration CFI did not resolve visually'));
              }
            }, 50);
          });
          completed.push({format:'epub-narration', segments:narration.length, chapters:2, cfiJump:true});
        }
        await new Promise(resolve => setTimeout(resolve, 300));
        const lateFailure = messages.find(message => message.type === 'error');
        if (lateFailure) throw new Error(fixture.format + ': ' + lateFailure.message);
        removeEventListener('message', receive);
        frame.remove();
      }
      if (pwned || errors.length) throw new Error('Reader isolation/lifecycle failure: ' + JSON.stringify(errors));
      webkit.messageHandlers.result.postMessage(JSON.stringify({ok:true, completed}));
    })().catch(error => webkit.messageHandlers.result.postMessage(JSON.stringify({ok:false, error:String(error), errors})));
    </script></body></html>`;
  const server = http.createServer((_request, response) => {
    response.setHeader("Content-Type", "text/html; charset=utf-8");
    response.end(html);
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const code = await new Promise((resolve, reject) => {
      const child = spawn("xvfb-run", ["-a", "/usr/bin/python3", path.join(__dirname, "books.webkit.py"),
        `http://127.0.0.1:${server.address().port}/`], {
        stdio: "inherit", env: { ...process.env, GDK_BACKEND: "x11", WEBKIT_DISABLE_COMPOSITING_MODE: "1" },
      });
      child.on("error", reject);
      child.on("exit", code => resolve(code ?? 1));
    });
    if (code !== 0) throw new Error(`Native WebKitGTK reader check exited with ${code}`);
  } finally {
    await new Promise(resolve => server.close(resolve));
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
