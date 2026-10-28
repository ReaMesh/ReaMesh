import { describe, expect, it } from "vitest";
import { decodeTicket, isDialableIp, isInviteLink, normalizeTicket, ticketAddrs } from "./ticket";

const PEER = "12D3KooWQYhTNQdmr3ArTeUHRYzFg94BKyTkoWBDWez9kSCVe2Xo";

/** A link the way the extension's `encode_ticket` writes it. */
function makeTicket(over: Partial<Record<"sid" | "n" | "s" | "p" | "a" | "t" | "q", string | null>> = {}) {
  const p = {
    sid: "abc123-9f",
    n: "Late%20Night%20Mix",
    s: "1",
    p: PEER,
    a: "192.168.1.7,100.101.102.103",
    t: "54001",
    q: "54002",
    ...over,
  };
  const params = (["n", "s", "p", "a", "t", "q"] as const)
    .filter((k) => p[k] !== null)
    .map((k) => `${k}=${p[k]}`)
    .join("&");
  return `reamesh://join/${p.sid}?${params}`;
}

describe("normalizeTicket", () => {
  it("strips embedded whitespace (line-wrapped links from chat/email)", () => {
    expect(normalizeTicket("reamesh://join/ab\ncd  ef\tgh\r\nij")).toBe("reamesh://join/abcdefghij");
  });
});

describe("isInviteLink", () => {
  it("recognises our scheme + path, case-insensitively", () => {
    expect(isInviteLink("reamesh://join/x")).toBe(true);
    expect(isInviteLink("ReaMesh://Join/x")).toBe(true);
    expect(isInviteLink("https://join/x")).toBe(false);
    expect(isInviteLink("reamesh://host/x")).toBe(false);
  });
});

describe("decodeTicket", () => {
  it("decodes a well-formed link", () => {
    const info = decodeTicket(makeTicket());
    expect(info).not.toBeNull();
    expect(info!.sid).toBe("abc123-9f");
    expect(info!.name).toBe("Late Night Mix");
    expect(info!.auth).toBe(true);
    expect(info!.peer).toBe(PEER);
  });

  it("reads an open session with the optional parts absent", () => {
    const info = decodeTicket(makeTicket({ n: null, s: null, q: null }));
    expect(info).toEqual({ sid: "abc123-9f", name: "", auth: false, peer: PEER, relays: 0 });
    // QUIC-only hosts are fine too.
    expect(decodeTicket(makeTicket({ t: null }))).not.toBeNull();
  });

  it("tolerates a line-wrapped paste, a capitalised scheme and a trailing slash", () => {
    const t = makeTicket();
    const wrapped = `${t.slice(0, 20)}\n${t.slice(20, 45)} ${t.slice(45)}`;
    expect(decodeTicket(wrapped)).not.toBeNull();
    expect(decodeTicket(t.replace("reamesh://", "ReaMesh://"))).not.toBeNull();
    expect(decodeTicket(t.replace("abc123-9f?", "abc123-9f/?"))).not.toBeNull();
  });

  it("keeps a garbled name cosmetic and ignores unknown parameters", () => {
    expect(decodeTicket(makeTicket({ n: "%ff%fe" }))!.name).toBe("");
    expect(decodeTicket(`${makeTicket()}&z=1`)).not.toBeNull();
    // IPv6 hosts (a Tailscale ULA, say) pass the loose address check.
    expect(decodeTicket(makeTicket({ a: "fd7a:115c:a1e0::1" }))).not.toBeNull();
  });

  it("rejects garbage the extension would refuse (no silent dead ends)", () => {
    expect(decodeTicket("")).toBeNull();
    expect(decodeTicket("reamesh://join/")).toBeNull();
    expect(decodeTicket("reamesh://join/abc")).toBeNull(); // no peer, no address
    expect(decodeTicket(makeTicket().replace("reamesh://join/", "https://join/"))).toBeNull();
    expect(decodeTicket(makeTicket().replace("reamesh://join/", "reamesh://host/"))).toBeNull();
    expect(decodeTicket(`reamesh://join/${"x".repeat(9000)}`)).toBeNull(); // over the cap
    expect(decodeTicket(makeTicket({ sid: "" }))).toBeNull();
    expect(decodeTicket(makeTicket({ sid: "../etc" }))).toBeNull(); // hostile id
    expect(decodeTicket(makeTicket({ p: "not-a-peer-id" }))).toBeNull();
    expect(decodeTicket(makeTicket({ p: null }))).toBeNull();
    expect(decodeTicket(makeTicket({ a: "nope" }))).toBeNull();
    expect(decodeTicket(makeTicket({ a: "999.1.1.1" }))).toBeNull();
    expect(decodeTicket(makeTicket({ a: null }))).toBeNull();
    expect(decodeTicket(makeTicket({ t: null, q: null }))).toBeNull(); // no port
    expect(decodeTicket(makeTicket({ t: "70000" }))).toBeNull();
    expect(decodeTicket(makeTicket({ t: "0" }))).toBeNull();
    expect(decodeTicket(makeTicket({ q: "abc" }))).toBeNull();
  });
});

describe("relays and the link secret", () => {
  const RELAY = "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN";
  const SECRET = "0123456789abcdef0123456789abcdef";

  it("counts the relays a link names and accepts a well-formed secret", () => {
    const t = `${makeTicket({ s: null })}&x=${SECRET}&r=${RELAY}~203.0.113.5~4001~4001~relay.example.org`;
    expect(decodeTicket(t)).toMatchObject({ auth: false, relays: 1 });
    // The same relay twice counts once; a garbled entry is skipped.
    expect(decodeTicket(`${t}&r=${RELAY}~203.0.113.6~4001~~&r=garbage`)!.relays).toBe(1);
  });

  it("reads a link that names only relays", () => {
    const t = `${makeTicket({ a: null, t: null, q: null })}&r=${RELAY}~~~4001~relay.example.org`;
    expect(decodeTicket(t)).toMatchObject({ relays: 1 });
    expect(ticketAddrs(t)).toEqual({ ips: [], tcp: 0, quic: 0 });
  });

  it("refuses what the extension refuses", () => {
    expect(decodeTicket(`${makeTicket()}&x=zz`)).toBeNull();
    expect(decodeTicket(`${makeTicket()}&x=${SECRET.toUpperCase()}`)).toBeNull();
    // No direct address and no usable relay: nothing to dial.
    const bare = makeTicket({ a: null, t: null, q: null });
    expect(decodeTicket(`${bare}&r=${RELAY}~~~~`)).toBeNull();
    expect(decodeTicket(`${bare}&r=${RELAY}~127.0.0.1~4001~~`)).toBeNull();
    expect(decodeTicket(`${bare}&r=${RELAY}~~4001~~bad_name`)).toBeNull();
  });
});

describe("ticketAddrs", () => {
  it("returns the host's dialable addresses for the Bluetooth advertisement", () => {
    expect(ticketAddrs(makeTicket())).toEqual({
      ips: ["192.168.1.7", "100.101.102.103"],
      tcp: 54001,
      quic: 54002,
    });
    expect(ticketAddrs(makeTicket({ q: null, a: "10.0.0.2,nope" }))).toEqual({
      ips: ["10.0.0.2"],
      tcp: 54001,
      quic: 0,
    });
    expect(ticketAddrs(makeTicket({ a: null }))).toBeNull();
  });
});

describe("isDialableIp", () => {
  it("accepts ordinary host addresses", () => {
    for (const ip of [
      "192.168.1.7",
      "10.0.0.2",
      "100.101.102.103",
      "fd7a:115c:a1e0::1",
      "2001:db8::5",
      "1:2:3:4:5:6:7:8",
      "::ffff:192.168.1.7",
    ]) {
      expect(isDialableIp(ip), ip).toBe(true);
    }
  });

  it("refuses local and non-host addresses", () => {
    for (const ip of [
      "127.0.0.1",
      "127.8.9.10",
      "0.0.0.0",
      "169.254.1.1",
      "224.0.0.251",
      "239.1.2.3",
      "255.255.255.255",
      "::1",
      "::",
      "0:0:0:0:0:0:0:1",
      "fe80::1",
      "FE80::abcd",
      "ff02::fb",
      "::ffff:127.0.0.1",
      "::ffff:169.254.1.1",
      "1::2::3",
      "12345::1",
      "1:2:3:4:5:6:7",
    ]) {
      expect(isDialableIp(ip), ip).toBe(false);
    }
  });

  it("drops them from a ticket, and refuses a ticket left with none", () => {
    expect(ticketAddrs(makeTicket({ a: "127.0.0.1,192.168.1.7,ff02::1" }))?.ips).toEqual([
      "192.168.1.7",
    ]);
    expect(decodeTicket(makeTicket({ a: "127.0.0.1" }))).toBeNull();
    expect(decodeTicket(makeTicket({ a: "::1,169.254.3.4" }))).toBeNull();
  });
});
