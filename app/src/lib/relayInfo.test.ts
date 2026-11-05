import { describe, expect, it } from "vitest";
import { noticeApplies, noticeLine, noticeWhen, relayNotes, relayVersionLine, type RelayNotice } from "./relayInfo";

const NOW = 1_800_000_000;
const DAY = 86_400;
const keys = (a: Parameters<typeof relayNotes>[0]) => relayNotes(a, NOW, "en").map((n) => n.key);

describe("noticeApplies", () => {
  const n = (from: number, until: number): RelayNotice => ({ text: "x", from, until });
  it("drops what is over or too far off", () => {
    expect(noticeApplies(null, NOW)).toBe(false);
    expect(noticeApplies({ text: "", from: 0, until: 0 }, NOW)).toBe(false);
    expect(noticeApplies(n(0, 0), NOW)).toBe(true);
    expect(noticeApplies(n(NOW - DAY, NOW - 1), NOW)).toBe(false);
    expect(noticeApplies(n(NOW + DAY, NOW + 2 * DAY), NOW)).toBe(true);
    expect(noticeApplies(n(NOW + 30 * DAY, 0), NOW)).toBe(false);
  });
});

describe("noticeWhen", () => {
  // Noon, far from any date line, so the test holds in every time zone.
  const noon = Date.UTC(2026, 10, 11, 12, 0) / 1000;
  it("names one day once", () => {
    const text = noticeWhen({ text: "x", from: noon, until: noon + 3600 }, "de");
    expect(text.match(/–/g)?.length).toBe(1);
    expect(text).not.toContain(" – ");
  });
  it("names both days when it spans more", () => {
    expect(noticeWhen({ text: "x", from: noon, until: noon + 2 * DAY }, "de")).toContain(" – ");
  });
  it("has nothing to say without times", () => {
    expect(noticeWhen({ text: "x", from: 0, until: 0 }, "de")).toBe("");
    expect(noticeLine({ text: "Umzug", from: 0, until: 0 }, "de")).toBe("Umzug");
  });
});

describe("relayNotes", () => {
  it("says nothing about a relay that is fine", () => {
    expect(keys(undefined)).toEqual([]);
    expect(keys({ mode: "full", budget_percent: 89, key_budget_percent: 89, key_expires: 0 })).toEqual([]);
  });
  it("puts what doesn't work first, the worst budget only", () => {
    expect(keys({ outdated: true, budget_exhausted: true, key_budget_exhausted: true, budget_percent: 100 })).toEqual([
      "relayOutdated",
      "relayBudgetUsedUp",
    ]);
    expect(keys({ key_budget_exhausted: true })).toEqual(["relayKeyBudgetUsedUp"]);
    expect(keys({ budget_percent: 90 })).toEqual(["relayBudgetNearly"]);
    expect(keys({ key_budget_percent: 95 })).toEqual(["relayKeyBudgetNearly"]);
  });
  it("mentions a coordination-only relay calmly", () => {
    const [note] = relayNotes({ mode: "coordination" }, NOW, "en");
    expect(note.key).toBe("relayCoordination");
    expect(note.warn).toBe(false);
  });
  it("mentions an expiring key only when it is close", () => {
    expect(keys({ key_expires: NOW + 30 * DAY })).toEqual([]);
    expect(keys({ key_expires: NOW + 3 * DAY })).toEqual(["relayKeyExpires"]);
    expect(keys({ key_expires: NOW - 1 })).toEqual([]);
  });
});

describe("relayVersionLine", () => {
  it("only offers an update when the relay is older", () => {
    expect(relayVersionLine({}, "1.2.0")).toBe("");
    const same = relayVersionLine({ version: "1.2.0" }, "1.2.0");
    const newer = relayVersionLine({ version: "1.3.0" }, "1.2.0");
    const older = relayVersionLine({ version: "1.1.0" }, "1.2.0");
    expect(same).toContain("1.2.0");
    expect(newer).toContain("1.3.0");
    expect(older).toContain("1.1.0");
    expect(older.length).toBeGreaterThan(same.length);
    expect(relayVersionLine({ version: "1.1.0" }, "")).not.toBe(older);
  });
});
