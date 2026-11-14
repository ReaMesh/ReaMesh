// Tiny i18n layer, no library. Every language is one JSON file in
// src/locales/ (`<BCP 47 code>.json`, a flat key → string map); dropping a
// file in is all it takes to add one. English is the source: it defines the
// keys, and a key a language lacks is shown in English.
//
// The preference ("system" or a language code) is persisted backend-side (like
// the REAPER folder) so the native menu and every webview window share one
// source of truth, and mirrored into localStorage for a synchronous first
// render. "system" resolves through the device languages; with no match it
// falls back to English.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import en from "../locales/en.json";

export type MsgKey = keyof typeof en;
/** A language code that has a file in src/locales/. */
export type Lang = string;
export type LangPref = "system" | Lang;

type Dict = Partial<Record<string, string>>;

const KEY = "reamesh.lang";
const FALLBACK = "en";

const dicts: Record<Lang, Dict> = {};
for (const [path, dict] of Object.entries(
  import.meta.glob<Dict>("../locales/*.json", { eager: true, import: "default" }),
)) {
  dicts[path.slice(path.lastIndexOf("/") + 1, -".json".length)] = dict;
}

/** Every shipped language under its own name, English first. */
export const LANGUAGES: { code: Lang; name: string }[] = Object.keys(dicts)
  .map((code) => ({ code, name: dicts[code].languageName || code }))
  .sort((a, b) =>
    a.code === FALLBACK ? -1 : b.code === FALLBACK ? 1 : a.name.localeCompare(b.name),
  );

/** The shipped language for a device tag: "pt-BR" matches pt-BR.json, else
 *  pt.json. */
export function matchLang(tag: string): Lang | undefined {
  const want = tag.toLowerCase();
  const codes = Object.keys(dicts);
  return (
    codes.find((c) => c.toLowerCase() === want) ??
    codes.find((c) => c.toLowerCase() === want.split("-")[0])
  );
}

function validPref(v: unknown): v is LangPref {
  return typeof v === "string" && (v === "system" || v in dicts);
}

function readStoredPref(): LangPref {
  try {
    const v = localStorage.getItem(KEY);
    if (validPref(v)) return v;
  } catch {
    // fall through
  }
  return "system";
}

let pref: LangPref = readStoredPref();
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

export function resolveLang(p: LangPref = pref): Lang {
  if (p !== "system") return p in dicts ? p : FALLBACK;
  const device = navigator.languages?.length ? navigator.languages : [navigator.language || ""];
  for (const tag of device) {
    const m = matchLang(tag);
    if (m) return m;
  }
  return FALLBACK;
}

const pluralRules: Record<Lang, Intl.PluralRules> = {};

function pluralCategory(lang: Lang, n: number): Intl.LDMLPluralRule {
  try {
    return (pluralRules[lang] ??= new Intl.PluralRules(lang)).select(n);
  } catch {
    return n === 1 ? "one" : "other";
  }
}

// A message with a `{count}` may have siblings for the language's plural
// categories: `<key>One`, `<key>Few`, `<key>Many`, … (`<key>` itself is the
// "other" form). English and German only ever need `<key>One`.
function lookup(lang: Lang, key: string, count: number | undefined): string | undefined {
  const dict = dicts[lang];
  if (!dict) return undefined;
  if (count !== undefined) {
    const cat = pluralCategory(lang, count);
    const form = cat === "other" ? "" : dict[key + cat[0].toUpperCase() + cat.slice(1)];
    if (form) return form;
  }
  return dict[key] || undefined;
}

export function t(key: MsgKey, params?: Record<string, string | number>): string {
  const n = params && "count" in params ? Number(params.count) : NaN;
  const count = Number.isFinite(n) ? n : undefined;
  let s = lookup(resolveLang(), key, count) ?? lookup(FALLBACK, key, count) ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) s = s.split(`{${k}}`).join(String(v));
  }
  return s;
}

export function getLangPref(): LangPref {
  return pref;
}

/** Set the preference, mirror it to localStorage, and persist it backend-side
 *  (where the native menu and the other windows read it). */
export function setLangPref(p: LangPref) {
  applyPref(p);
  invoke("set_language", { pref: p }).catch(() => {});
}

function applyPref(p: LangPref) {
  if (p === pref) return;
  pref = p;
  try {
    localStorage.setItem(KEY, p);
  } catch {
    // localStorage is unavailable, so keep the in-memory value.
  }
  syncDocumentLang();
  emit();
}

/** Set the document language so screen readers use the UI's pronunciation rules. */
function syncDocumentLang() {
  if (typeof document !== "undefined") document.documentElement.lang = resolveLang();
}

/** Subscribe a component to language changes; returns the resolved language. */
export function useLang(): Lang {
  useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => pref,
  );
  return resolveLang();
}

/** One-time backend sync: adopt the persisted preference (the backend file is
 *  the source of truth because another window may have changed it while this
 *  one was gone) and follow later changes live. No-ops in
 *  plain-browser dev where the Tauri APIs reject. */
export function initLanguage() {
  syncDocumentLang();
  invoke<string | null>("get_language")
    .then((v) => {
      if (validPref(v)) applyPref(v);
    })
    .catch(() => {});
  listen<string>("language-changed", (ev) => {
    if (validPref(ev.payload)) applyPref(ev.payload);
  }).catch(() => {});
}
