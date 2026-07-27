// Browser shims for tests running in Node: localStorage and navigator.language.
// These tests do not render components, so they do not need a DOM.

const store = new Map<string, string>();
const localStorageShim = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => {
    store.set(k, String(v));
  },
  removeItem: (k: string) => {
    store.delete(k);
  },
  clear: () => {
    store.clear();
  },
};

// Probe usability, not existence: Node exposes a `localStorage` global that
// is an unusable stub unless started with --experimental-webstorage.
let usable = false;
try {
  const ls = (globalThis as { localStorage?: Storage }).localStorage;
  if (ls && typeof ls.setItem === "function" && typeof ls.removeItem === "function") {
    ls.setItem("__probe__", "1");
    ls.removeItem("__probe__");
    usable = true;
  }
} catch {
  usable = false;
}
if (!usable) {
  try {
    Object.defineProperty(globalThis, "localStorage", {
      value: localStorageShim,
      configurable: true,
    });
  } catch {
    // frozen global: the libs under test guard with try/catch anyway
  }
}

try {
  // Node ≥21 ships a (frozen) navigator without `language`; older ones have
  // none at all. Either way the tests want a deterministic "en".
  const nav = (globalThis as { navigator?: { language?: string } }).navigator;
  if (!nav) {
    Object.defineProperty(globalThis, "navigator", { value: { language: "en" } });
  } else if (!nav.language) {
    Object.defineProperty(nav, "language", { value: "en", configurable: true });
  }
} catch {
  // frozen navigator with a language already set: deterministic enough
}
