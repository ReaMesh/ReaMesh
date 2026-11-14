// Checks every language file in src/locales against the English source, so a
// translation can't ship a key the app doesn't know or a placeholder the code
// never fills. A missing key is fine (it shows in English) and only reported.

import { describe, expect, it } from "vitest";
import en from "../locales/en.json";
import { LANGUAGES, matchLang, setLangPref, t } from "./i18n";

const source: Record<string, string> = en;
const files = import.meta.glob<Record<string, string>>("../locales/*.json", {
  eager: true,
  import: "default",
});
const locales = Object.entries(files).map(
  ([path, dict]) => [path.slice(path.lastIndexOf("/") + 1, -".json".length), dict] as const,
);

/** Languages the project maintains itself: these must be complete. */
const MAINTAINED = ["en", "de"];

const PLURAL = /(Zero|One|Two|Few|Many)$/;

/** The English key a translated key stands for: itself, or the `{count}`
 *  message it is a plural form of. */
function baseKey(key: string): string | undefined {
  if (key in source) return key;
  const base = key.replace(PLURAL, "");
  return base !== key && base in source && source[base].includes("{count}") ? base : undefined;
}

const placeholders = (s: string) => new Set(s.match(/\{[A-Za-z0-9_]+\}/g) ?? []);

describe.each(locales)("locale %s", (code, dict) => {
  it("is named by a valid language code", () => {
    expect(Intl.getCanonicalLocales(code)).toEqual([code]);
    expect(dict.languageName?.trim()).toBeTruthy();
  });

  it("holds only strings under keys the app knows", () => {
    const unknown = Object.keys(dict).filter((k) => baseKey(k) === undefined);
    expect(unknown).toEqual([]);
    expect(Object.values(dict).filter((v) => typeof v !== "string")).toEqual([]);
  });

  it("uses only the placeholders of the English text", () => {
    const wrong: string[] = [];
    for (const [key, text] of Object.entries(dict)) {
      const base = baseKey(key);
      if (!base) continue;
      // A plural form may show the number, whatever the English one does.
      const allowed = placeholders(source[base]);
      if (key in source) for (const p of placeholders(source[key])) allowed.add(p);
      for (const p of placeholders(text)) if (!allowed.has(p)) wrong.push(`${key}: ${p}`);
    }
    expect(wrong).toEqual([]);
  });

  it("reports what is still in English", () => {
    const missing = Object.keys(source).filter((k) => !dict[k]);
    if (MAINTAINED.includes(code)) expect(missing).toEqual([]);
    else if (missing.length) console.warn(`${code}: ${missing.length} of ${Object.keys(source).length} messages untranslated`);
  });
});

describe("language selection", () => {
  it("lists every file, English first", () => {
    expect(LANGUAGES.map((l) => l.code).sort()).toEqual(locales.map(([c]) => c).sort());
    expect(LANGUAGES[0]).toEqual({ code: "en", name: "English" });
  });

  it("matches a device tag by region, then by language", () => {
    expect(matchLang("de-AT")).toBe("de");
    expect(matchLang("EN-us")).toBe("en");
    expect(matchLang("xx-YY")).toBeUndefined();
  });

  it("picks the plural form of the language", () => {
    setLangPref("en");
    expect(t("daysLeft", { count: 1 })).toBe("1 day left");
    expect(t("daysLeft", { count: 3 })).toBe("3 days left");
    setLangPref("de");
    expect(t("stopHostingBody", { count: 1 })).toBe("Eine Person wird getrennt.");
    expect(t("stopHostingBody", { count: 2 })).toBe("2 Personen werden getrennt.");
    setLangPref("en");
  });
});
