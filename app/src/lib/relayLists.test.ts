import { describe, expect, it } from "vitest";
import { hostListNote, hostVisibleText, mergeTicket, relaySections, viaNames } from "./relayLists";
import type { RelayCode } from "./relay";
import type { RelayList, RelayListEntry, SessionAd } from "./types";
import { setLangPref } from "./i18n";

const relay = (peer: string, label: string): RelayCode => ({
  code: `reamesh://relay/${peer}`,
  peer,
  ips: ["203.0.113.5"],
  dns: "",
  tcp: 4001,
  quic: 4001,
  label,
  list: false,
});

const entry = (over: Partial<RelayListEntry> = {}): RelayListEntry => ({
  session_id: "s1",
  session_name: "Podcast",
  owner: "lena",
  owner_name: "Lena",
  owner_color: "#ee7fcf",
  auth: false,
  files: 2,
  bytes: 100,
  proto: 7,
  version: "1.2.0",
  ticket: "reamesh://join/s1?p=lena&a=&x=aa&r=band~1.1.1.1~1~1",
  key_label: "lena",
  ...over,
});

const list = (over: Partial<RelayList> = {}): RelayList => ({
  state: "ok",
  reason: "",
  enabled: true,
  budget_used: false,
  count: 0,
  entries: [],
  ...over,
});

const lan = (over: Partial<SessionAd> = {}): SessionAd => ({
  session_id: "s1",
  session_name: "Podcast",
  project_file: "p.rpp",
  owner: "lena",
  auth: false,
  ...over,
});

const band = relay("band", "Bandrelay");
const studio = relay("studio", "Studio");

describe("relaySections", () => {
  it("shows nothing for a relay without sessions", () => {
    const { sections } = relaySections([band], { band: list() }, []);
    expect(sections).toEqual([]);
  });

  it("shows nothing for a relay without a list, whatever it's doing", () => {
    const lists = { band: list({ enabled: false, entries: [entry()] }), studio: list({ enabled: false, state: "down" }) };
    expect(relaySections([band, studio], lists, []).sections).toEqual([]);
  });

  it("keeps a relay's section once someone hosts there, in the settings' order", () => {
    const lists = {
      band: list({ entries: [entry()] }),
      studio: list({ entries: [entry({ session_id: "s2", owner: "noa" })] }),
    };
    const { sections } = relaySections([studio, band], lists, []);
    expect(sections.map((s) => s.name)).toEqual(["Studio", "Bandrelay"]);
    expect(sections[1].rows[0].session.session_name).toBe("Podcast");
    expect(sections[1].rows[0].session.project_file).toBe("");
  });

  it("drops the empty relay and keeps the full one", () => {
    const lists = { band: list({ entries: [entry()] }), studio: list() };
    const { sections } = relaySections([band, studio], lists, []);
    expect(sections.map((s) => s.name)).toEqual(["Bandrelay"]);
  });

  it("says when a relay with a list has a problem, even without sessions", () => {
    const lists = {
      band: list({ state: "down", entries: [] }),
      studio: list({ state: "refused", reason: "bad_key" }),
    };
    const { sections } = relaySections([band, studio], lists, []);
    expect(sections.map((s) => s.problem)).toEqual(["down", "refused"]);
    const expired = relaySections([band], { band: list({ state: "refused", reason: "expired" }) }, []);
    expect(expired.sections[0].problem).toBe("expired");
  });

  it("stays quiet while connecting", () => {
    expect(relaySections([band], { band: list({ state: "connecting" }) }, []).sections).toEqual([]);
  });

  it("carries the used-up budget with the section", () => {
    const { sections } = relaySections([band], { band: list({ budget_used: true, entries: [entry()] }) }, []);
    expect(sections[0].budgetUsed).toBe(true);
  });

  it("lets the local network win over a relay", () => {
    const { sections, contested } = relaySections([band], { band: list({ entries: [entry()] }) }, [lan()]);
    expect(sections).toEqual([]);
    expect(contested.size).toBe(0);
  });

  it("marks a session two hosts claim, on the relay row and for the local one", () => {
    const { sections, contested } = relaySections(
      [band],
      { band: list({ entries: [entry({ owner: "mallory" })] }) },
      [lan()],
    );
    expect(contested.has("s1")).toBe(true);
    expect(sections[0].rows[0].session.contested).toBe(true);
  });

  it("shows a session on two relays once, with both ways in", () => {
    const lists = {
      band: list({ entries: [entry()] }),
      studio: list({ entries: [entry({ ticket: "reamesh://join/s1?p=lena&a=&x=aa&r=studio~2.2.2.2~1~1", key_label: "other" })] }),
    };
    const { sections } = relaySections([band, studio], lists, []);
    expect(sections).toHaveLength(1);
    const row = sections[0].rows[0];
    expect(row.via).toEqual(["Bandrelay", "Studio"]);
    expect(row.keyLabel).toBe("lena");
    expect(row.ticket).toContain("r=band~");
    expect(row.ticket).toContain("r=studio~");
  });

  it("marks one session id with two hosts on two relays", () => {
    const lists = {
      band: list({ entries: [entry()] }),
      studio: list({ entries: [entry({ owner: "mallory" })] }),
    };
    const { sections } = relaySections([band, studio], lists, []);
    expect(sections).toHaveLength(1);
    expect(sections[0].rows[0].session.contested).toBe(true);
    expect(sections[0].rows[0].session.owner).toBe("lena");
  });
});

describe("mergeTicket", () => {
  it("adds only the relays it doesn't name yet", () => {
    const a = "reamesh://join/s1?p=x&a=&r=band~1~1~1";
    expect(mergeTicket(a, "reamesh://join/s1?p=x&a=&r=band~1~1~1")).toBe(a);
    expect(mergeTicket(a, "reamesh://join/s1?p=x&a=&r=studio~2~1~1")).toBe(`${a}&r=studio~2~1~1`);
  });
});

describe("the host card's line", () => {
  it("names where the session shows, in German and English", () => {
    setLangPref("de");
    expect(hostVisibleText([band], [])).toBe("Sichtbar in Bandrelay");
    expect(hostVisibleText([band, studio], [])).toBe("Sichtbar in Bandrelay und Studio");
    expect(hostVisibleText([band, studio], ["band"])).toBe("Sichtbar in Studio");
    expect(hostVisibleText([band, studio], ["band", "studio"])).toBe("In keiner Relay-Liste");
    expect(hostVisibleText([band, studio, relay("x", "Proberaum")], [])).toBe(
      "Sichtbar in Bandrelay, Studio und Proberaum",
    );
    expect(viaNames(["Bandrelay", "Studio"])).toBe("Bandrelay und Studio");
    setLangPref("en");
    expect(hostVisibleText([band], [])).toBe("Visible on Bandrelay");
    expect(hostVisibleText([band, studio], [])).toBe("Visible on Bandrelay and Studio");
    expect(hostVisibleText([band, studio], ["band", "studio"])).toBe("Not on any relay list");
    expect(viaNames(["Bandrelay", "Studio"])).toBe("Bandrelay and Studio");
  });

  it("explains itself by password and choice", () => {
    setLangPref("de");
    expect(hostListNote([band], [], false)).toBe("Wer dort einen Code hat, sieht die Session und kann beitreten.");
    expect(hostListNote([band], [], true)).toBe(
      "Wer dort einen Code hat, sieht die Session. Zum Beitreten braucht es das Passwort.",
    );
    expect(hostListNote([band], ["band"], true)).toBe(
      "Die Session bleibt aus den Listen. Einladen geht weiter per Code.",
    );
  });
});
