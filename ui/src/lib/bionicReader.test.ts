import { describe, expect, it } from "vitest";
import {
  applyBionicReaderToElement,
  bionicPrefixLength,
  removeBionicReaderFromElement,
} from "./bionicReader";

describe("bionic reader", () => {
  it("bolds the leading part of readable words", () => {
    document.body.innerHTML = `<div id="root">Reading faster with Grafium</div>`;
    const root = document.getElementById("root")!;

    applyBionicReaderToElement(root);

    expect(root.querySelectorAll(".bionic-word-prefix").length).toBe(4);
    expect(root.textContent).toBe("Reading faster with Grafium");
  });

  it("skips code and can unwrap cleanly", () => {
    document.body.innerHTML = `<div id="root">Normal <code>const value = 1</code></div>`;
    const root = document.getElementById("root")!;

    applyBionicReaderToElement(root);
    expect(root.querySelector("code .bionic-word-prefix")).toBeNull();

    removeBionicReaderFromElement(root);
    expect(root.querySelector(".bionic-word-prefix")).toBeNull();
    expect(root.textContent).toBe("Normal const value = 1");
  });

  it("uses short prefixes for short words", () => {
    expect(bionicPrefixLength("to")).toBe(1);
    expect(bionicPrefixLength("reading")).toBe(3);
  });

  it("preserves empty text anchors owned by the renderer", () => {
    const root = document.createElement("div");
    root.innerHTML = "<table><tbody><tr><td>Reading notes</td></tr></tbody></table>";
    const anchor = document.createTextNode("");
    root.append(anchor);

    applyBionicReaderToElement(root);
    removeBionicReaderFromElement(root);

    expect(root.lastChild).toBe(anchor);
    expect(anchor.parentNode).toBe(root);
    expect(root.querySelector("td")?.textContent).toBe("Reading notes");
  });
});
