import { beforeEach, describe, expect, it } from "vitest";
import { inkFor } from "./colors";
import { formatBytes } from "./format";
import { hostSeenCount, recordHostJoin } from "./knownHosts";
import { localizeLogEntry } from "./log";
import { initials, normalizeState } from "./types";

describe("initials", () => {
  it("takes first+last word initials, whole code points", () => {
    expect(initials("Daniel Weiss")).toBe("DW");
    expect(initials("daniel")).toBe("DA");
    expect(initials("  ")).toBe("?");
    expect(initials("Ana Maria Silva")).toBe("AS");
  });
  it("never splits an astral character into surrogates", () => {
    expect(initials("🎸 Player")).toBe("🎸P");
    expect(initials("🎸🎤")).toBe("🎸🎤");
  });
});

describe("formatBytes", () => {
  it("picks sensible units", () => {
    expect(formatBytes(0, "en")).toBe("0 B");
    expect(formatBytes(999, "en")).toBe("999 B");
    expect(formatBytes(1_000, "en")).toBe("1 kB");
    expect(formatBytes(412_000_000, "en")).toBe("412 MB");
    expect(formatBytes(1_300_000_000, "en")).toBe("1.30 GB");
  });
  it("localizes the decimal separator", () => {
    expect(formatBytes(1_300_000_000, "de")).toBe("1,30 GB");
  });
});

describe("inkFor", () => {
  it("puts dark ink on light backgrounds and light ink on dark ones", () => {
    expect(inkFor("#e8d35c")).toBe("rgba(0,0,0,0.82)"); // gold → dark text
    expect(inkFor("#101013")).toBe("#ffffff"); // near-black → light text
    expect(inkFor(undefined)).toBe("rgba(0,0,0,0.8)"); // unparseable → default
    expect(inkFor("#abc")).toBe(inkFor("#aabbcc")); // short hex expands
  });
});

describe("normalizeState", () => {
  it("forces the array fields to arrays under version skew", () => {
    const s = normalizeState({ mode: "client", peers: null, sessions: "x", log: undefined });
    expect(s.peers).toEqual([]);
    expect(s.sessions).toEqual([]);
    expect(s.log).toEqual([]);
  });
  it("keeps real arrays intact", () => {
    const s = normalizeState({ mode: "client", peers: [{ id: "a" }], sessions: [], log: [] });
    expect(s.peers).toHaveLength(1);
  });
});

describe("knownHosts", () => {
  // removeItem, not clear(): the runtime's built-in localStorage (Node's
  // experimental Web Storage) may not implement clear().
  beforeEach(() => localStorage.removeItem("reamesh.knownHosts"));
  it("counts joins per host peer id", () => {
    expect(hostSeenCount("peerA")).toBe(0);
    recordHostJoin("peerA");
    expect(hostSeenCount("peerA")).toBe(1);
    recordHostJoin("peerA");
    recordHostJoin("peerB");
    expect(hostSeenCount("peerA")).toBe(2);
    expect(hostSeenCount("peerB")).toBe(1);
    expect(hostSeenCount("")).toBe(0);
  });
  it("survives corrupted storage", () => {
    localStorage.setItem("reamesh.knownHosts", "{not json");
    expect(hostSeenCount("peerA")).toBe(0);
    recordHostJoin("peerA");
    expect(hostSeenCount("peerA")).toBe(1);
  });
});

describe("localizeLogEntry", () => {
  it("renders progressive-join events by code", () => {
    expect(
      localizeLogEntry({ code: "progressive_open", params: { count: 12 }, text: "fallback" }),
    ).toContain("12");
    expect(
      localizeLogEntry({ code: "loading_readonly", text: "fallback" }),
    ).not.toBe("fallback");
    expect(localizeLogEntry({ code: "media_ready", text: "fallback" })).not.toBe("fallback");
  });
  it("renders plugin arrivals with whatever context is known", () => {
    expect(
      localizeLogEntry({
        code: "plugin_added",
        params: { plugin: "Pro-Q 3", track: "Drums", by: "Dan" },
        text: "fallback",
      }),
    ).toContain("Pro-Q 3");
    expect(
      localizeLogEntry({ code: "plugin_added", params: { plugin: "Serum" }, text: "f" }),
    ).toContain("Serum");
    expect(
      localizeLogEntry({ code: "plugins_added", params: { count: 7, track: "Bus" }, text: "f" }),
    ).toContain("7");
  });
  it("renders shared markers with and without a sharer", () => {
    expect(
      localizeLogEntry({ code: "marker_received", params: { name: "Chorus", by: "Ana" }, text: "f" }),
    ).toContain("Ana");
    expect(
      localizeLogEntry({ code: "marker_received", params: { name: "Chorus" }, text: "f" }),
    ).toContain("Chorus");
  });
  it("falls back to the wire text for unknown codes", () => {
    expect(localizeLogEntry({ code: "brand_new_code", text: "raw english" })).toBe("raw english");
    expect(localizeLogEntry("plain string")).toBe("plain string");
  });
});
