import { afterEach, describe, expect, it, vi } from "vitest";
import { enclosingTopLayer } from "./topLayer";

function build(html: string): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = html;
  document.body.append(root);
  return root;
}

const at = (root: ParentNode, selector: string) => root.querySelector<HTMLElement>(selector)!;

/** jsdom has no top layer, so say which elements the engine would report as in it. */
function stubTopLayer(known: { modal?: Element[]; popover?: Element[] }) {
  const real = Element.prototype.matches;
  vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, selector: string) {
    if (selector === ":modal") return known.modal?.includes(this) ?? false;
    if (selector === ":popover-open") return known.popover?.includes(this) ?? false;
    return real.call(this, selector);
  });
}

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("enclosingTopLayer", () => {
  it("finds nothing in the ordinary page", () => {
    const root = build(`<main><input id="field"></main>`);
    expect(enclosingTopLayer(at(root, "#field"))).toBeNull();
    expect(enclosingTopLayer(null)).toBeNull();
    expect(enclosingTopLayer(undefined)).toBeNull();
  });

  it("finds the modal dialog around the element", () => {
    const root = build(`<main><dialog open><div><input id="field"></div></dialog></main>`);
    const dialog = at(root, "dialog");
    stubTopLayer({ modal: [dialog] });
    expect(enclosingTopLayer(at(root, "#field"))).toBe(dialog);
  });

  it("finds the dialog when the dialog itself is the target, as for a click on its backdrop", () => {
    const root = build(`<dialog open></dialog>`);
    const dialog = at(root, "dialog");
    stubTopLayer({ modal: [dialog] });
    expect(enclosingTopLayer(dialog)).toBe(dialog);
  });

  it("ignores a dialog that is not modal, which stays in the ordinary page", () => {
    const root = build(`<dialog open><input id="field"></dialog>`);
    stubTopLayer({ modal: [] });
    expect(enclosingTopLayer(at(root, "#field"))).toBeNull();
  });

  it("ignores a dialog that is closed", () => {
    const root = build(`<dialog><input id="field"></dialog>`);
    stubTopLayer({ modal: [] });
    expect(enclosingTopLayer(at(root, "#field"))).toBeNull();
  });

  it("finds an open popover but not a closed one", () => {
    const root = build(`<div popover id="shown"><p id="a"></p></div><div popover id="hidden"><p id="b"></p></div>`);
    stubTopLayer({ popover: [at(root, "#shown")] });
    expect(enclosingTopLayer(at(root, "#a"))).toBe(at(root, "#shown"));
    expect(enclosingTopLayer(at(root, "#b"))).toBeNull();
  });

  it("does not take an ordinary element for a popover just because it is open-ish", () => {
    const root = build(`<details open><summary id="s"></summary></details>`);
    stubTopLayer({});
    expect(enclosingTopLayer(at(root, "#s"))).toBeNull();
  });

  it("prefers the innermost layer: a popover inside a dialog", () => {
    const root = build(`<dialog open><div popover id="pop"><span id="deep"></span></div></dialog>`);
    const dialog = at(root, "dialog");
    stubTopLayer({ modal: [dialog], popover: [at(root, "#pop")] });
    expect(enclosingTopLayer(at(root, "#deep"))).toBe(at(root, "#pop"));
  });

  it("falls back to the open attribute where :modal is unknown", () => {
    const root = build(`<dialog open id="o"><b id="a"></b></dialog><dialog id="c"><b id="b"></b></dialog>`);
    const real = Element.prototype.matches;
    vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, selector: string) {
      if (selector === ":modal") throw new DOMException("not a valid selector", "SyntaxError");
      return real.call(this, selector);
    });
    expect(enclosingTopLayer(at(root, "#a"))).toBe(at(root, "#o"));
    expect(enclosingTopLayer(at(root, "#b"))).toBeNull();
  });

  it("treats a popover as closed where :popover-open is unknown", () => {
    const root = build(`<div popover><b id="a"></b></div>`);
    const real = Element.prototype.matches;
    vi.spyOn(Element.prototype, "matches").mockImplementation(function (this: Element, selector: string) {
      if (selector === ":popover-open") throw new DOMException("not a valid selector", "SyntaxError");
      return real.call(this, selector);
    });
    expect(enclosingTopLayer(at(root, "#a"))).toBeNull();
  });
});
