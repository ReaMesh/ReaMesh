import { describe, expect, it } from "vitest";
import { decodeTicket, isInviteLink, normalizeTicket } from "./ticket";

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
    expect(info).toEqual({ sid: "abc123-9f", name: "", auth: false, peer: PEER });
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
    expect(decodeTicket(`${makeTicket()}&x=1`)).not.toBeNull();
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
