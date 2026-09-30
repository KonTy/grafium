import { build, transform } from "esbuild";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const output = path.join(root, "public/book-reader");
await mkdir(output, { recursive: true });
const pdfRoot = path.join(root, "node_modules/pdfjs-dist");
const assets = {};
for (const dir of ["cmaps", "standard_fonts", "wasm"]) {
  for (const name of await readdir(path.join(pdfRoot, dir))) {
    if (/\.(bcmap|pfb|ttf|wasm)$/.test(name))
      assets[name] = (await readFile(path.join(pdfRoot, dir, name))).toString("base64");
    if (name.startsWith("LICENSE"))
      await writeFile(path.join(output, `${dir}-${name}.txt`), await readFile(path.join(pdfRoot, dir, name)));
  }
}
for (const [name, licensePath] of [
  ["Foliate", "vendor/foliate-js/LICENSE"],
  ["PDFjs", "node_modules/pdfjs-dist/LICENSE"],
  ["DOMPurify", "node_modules/dompurify/LICENSE"],
  ["zipjs", "node_modules/@zip.js/zip.js/LICENSE"],
  ["fflate", "node_modules/fflate/LICENSE"],
  ["noble-hashes", "node_modules/@noble/hashes/LICENSE"],
]) await writeFile(path.join(output, `${name}-LICENSE.txt`), await readFile(path.join(root, licensePath)));
const pdfWorker = await transform(await readFile(path.join(pdfRoot, "build/pdf.worker.mjs"), "utf8"), {
  minify: true, format: "iife", target: "es2022", legalComments: "eof",
});
await build({
  absWorkingDir: root, entryPoints: ["src/lib/reader/runtime.js"],
  outfile: "public/book-reader/runtime.js", bundle: true, format: "iife",
  target: "es2022", minify: true, legalComments: "eof",
  define: {
    PDF_WORKER_SOURCE: JSON.stringify(pdfWorker.code),
    PDF_LOCAL_ASSETS: JSON.stringify(assets),
  },
  plugins: [{
    name: "pinned-foliate-isolation",
    setup(build) {
      build.onResolve({ filter: /\/vendor\/zip\.js$/ }, () => ({ path: path.join(root, "node_modules/@zip.js/zip.js/index.js") }));
      build.onResolve({ filter: /\/vendor\/fflate\.js$/ }, () => ({ path: path.join(root, "node_modules/fflate/esm/browser.js") }));
      // Foliate's convenience makeBook includes a PDF adapter. It is deliberately never used.
      build.onResolve({ filter: /^\.\/pdf\.js$/ }, args => args.importer.includes("foliate-js")
        ? { path: "unused-foliate-pdf", namespace: "empty-pdf" } : undefined);
      build.onLoad({ filter: /.*/, namespace: "empty-pdf" }, () => ({
        contents: 'export function makePDF() { throw new Error("Use the direct PDF.js adapter"); }',
      }));
      build.onLoad({ filter: /foliate-js\/(paginator|fixed-layout)\.js$/ }, async ({ path: file }) => {
        let contents = await readFile(file, "utf8");
        // Blob navigations from an opaque data origin are cross-origin. srcdoc inherits
        // the isolated runtime origin instead; sanitization already happened in section.load.
        const paginator = file.endsWith("paginator.js");
        const before = paginator ? "return new Promise(resolve => {\n            this.#iframe.addEventListener"
          : "return new Promise(resolve => {\n            iframe.addEventListener";
        const assignment = paginator ? "this.#iframe.src = src" : "iframe.src = src";
        if (!contents.includes(before) || !contents.includes(assignment))
          throw new Error("Pinned Foliate iframe patch no longer matches. Re-audit before updating.");
        contents = contents.replace(before, `const sourceHTML = await (await fetch(src)).text()\n        ${before}`)
          .replace(assignment, `${paginator ? "this.#iframe" : "iframe"}.srcdoc = sourceHTML`);
        const frame = paginator ? "this.#iframe" : "iframe";
        const getDocument = paginator ? "const doc = this.document" : "const doc = iframe.contentDocument";
        contents = contents.replace(`${frame}.addEventListener('load', () => {`, "const onLoad = () => {")
          .replace(`${getDocument}\n                `, `${getDocument}\n                if (!doc?.body || !doc.querySelector('meta[http-equiv="Content-Security-Policy"]')) return\n                ${frame}.removeEventListener('load', onLoad)\n                `)
          .replace(`}, { once: true })\n            ${frame}.srcdoc`, `}\n            ${frame}.addEventListener('load', onLoad)\n            ${frame}.srcdoc`);
        if (paginator) contents = contents
          .replace("expand() {\n        const { documentElement }",
            "expand() {\n        if (!this.#iframe.isConnected || !this.document?.body) return\n        const { documentElement }")
          .replace("if (!this.#view) return\n        this.#view.render",
            "if (!this.#view?.document?.body || !this.#styleMap.has(this.#view.document)) return\n        this.#view.render")
          .replace("requestAnimationFrame(() =>\n            this.#background.style.background = getBackground(this.#view.document))",
            "const renderedDoc = this.#view?.document\n        requestAnimationFrame(() => {\n            if (renderedDoc?.body && renderedDoc === this.#view?.document) this.#background.style.background = getBackground(renderedDoc)\n        })")
          .replace("this.#view?.document?.fonts?.ready?.then(() => this.#view.expand())",
            "const renderedView = this.#view\n        renderedDoc?.fonts?.ready?.then(() => { if (renderedView === this.#view) renderedView?.expand() })");
        return { contents, loader: "js" };
      });
    },
  }],
});
console.log("Built isolated offline book reader and dependency licenses.");
