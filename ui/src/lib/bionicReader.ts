import { writable } from "svelte/store";

const STORAGE_KEY = "grafium.reader.bionic";
const WORD_RE = /[\p{L}\p{M}\p{N}][\p{L}\p{M}\p{N}'’_-]*/gu;
const SKIP_TAGS = new Set(["CODE", "PRE", "SCRIPT", "STYLE", "TEXTAREA", "INPUT", "KBD", "SAMP"]);
const SKIP_SELECTOR = [
  ".bionic-word",
  ".code-block-wrapper",
  ".katex",
  ".grafium-icon",
  ".task-checkbox",
  ".task-marker",
  ".priority",
  ".code-lang",
].join(",");

export const bionicReaderEnabled = writable(false);
let currentEnabled = false;

bionicReaderEnabled.subscribe((enabled) => {
  currentEnabled = enabled;
  if (typeof document !== "undefined") {
    document.documentElement.classList.toggle("bionic-reader-enabled", enabled);
  }
});

export function isBionicReaderEnabled(): boolean {
  return currentEnabled;
}

export function setBionicReaderEnabled(enabled: boolean): void {
  bionicReaderEnabled.set(enabled);
  try {
    localStorage.setItem(STORAGE_KEY, enabled ? "1" : "0");
  } catch (error) {
    console.warn("Could not save the Bionic reader preference; using it for this session only.", error);
  }
}

export function loadBionicReaderPreference(): boolean {
  let enabled = false;
  try {
    enabled = localStorage.getItem(STORAGE_KEY) === "1";
  } catch (error) {
    console.warn("Could not load the Bionic reader preference; starting with it disabled.", error);
  }
  bionicReaderEnabled.set(enabled);
  return enabled;
}

export function bionicPrefixLength(word: string): number {
  const chars = Array.from(word);
  if (chars.length <= 1) return chars.length;
  if (chars.length <= 3) return 1;
  return Math.max(1, Math.ceil(chars.length * 0.42));
}

function shouldSkipTextNode(node: Text, root: HTMLElement): boolean {
  if (!node.data.trim()) return true;
  let element = node.parentElement;
  while (element && element !== root) {
    if (SKIP_TAGS.has(element.tagName) || element.matches(SKIP_SELECTOR)) {
      return true;
    }
    element = element.parentElement;
  }
  return false;
}

function bionicFragment(text: string, ownerDocument: Document): DocumentFragment {
  const fragment = ownerDocument.createDocumentFragment();
  let index = 0;
  for (const match of text.matchAll(WORD_RE)) {
    const word = match[0];
    const start = match.index ?? 0;
    if (start > index) {
      fragment.append(ownerDocument.createTextNode(text.slice(index, start)));
    }

    const prefixLength = bionicPrefixLength(word);
    if (prefixLength >= Array.from(word).length) {
      fragment.append(ownerDocument.createTextNode(word));
    } else {
      const chars = Array.from(word);
      const wrapper = ownerDocument.createElement("span");
      wrapper.className = "bionic-word";
      wrapper.dataset.bionicWord = "1";

      const prefix = ownerDocument.createElement("strong");
      prefix.className = "bionic-word-prefix";
      prefix.textContent = chars.slice(0, prefixLength).join("");
      wrapper.append(prefix, ownerDocument.createTextNode(chars.slice(prefixLength).join("")));
      fragment.append(wrapper);
    }

    index = start + word.length;
  }

  if (index < text.length) {
    fragment.append(ownerDocument.createTextNode(text.slice(index)));
  }
  return fragment;
}

export function removeBionicReaderFromElement(root: HTMLElement): void {
  for (const wrapper of Array.from(root.querySelectorAll<HTMLElement>(".bionic-word[data-bionic-word='1']"))) {
    wrapper.replaceWith(root.ownerDocument.createTextNode(wrapper.textContent ?? ""));
  }
  root.normalize();
}

export function applyBionicReaderToElement(root: HTMLElement): void {
  removeBionicReaderFromElement(root);

  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) => shouldSkipTextNode(node as Text, root)
      ? NodeFilter.FILTER_REJECT
      : NodeFilter.FILTER_ACCEPT,
  });

  const textNodes: Text[] = [];
  while (walker.nextNode()) {
    textNodes.push(walker.currentNode as Text);
  }

  for (const textNode of textNodes) {
    const fragment = bionicFragment(textNode.data, root.ownerDocument);
    textNode.replaceWith(fragment);
  }
}

export function bionicReader(node: HTMLElement, _contentKey = "") {
  let frame = 0;

  const apply = () => {
    frame = 0;
    if (currentEnabled) {
      applyBionicReaderToElement(node);
    } else {
      removeBionicReaderFromElement(node);
    }
  };

  const schedule = () => {
    if (frame) cancelAnimationFrame(frame);
    frame = requestAnimationFrame(apply);
  };

  const unsubscribe = bionicReaderEnabled.subscribe(schedule);
  schedule();

  return {
    update() {
      schedule();
    },
    destroy() {
      unsubscribe();
      if (frame) cancelAnimationFrame(frame);
      removeBionicReaderFromElement(node);
    },
  };
}
