import { describe, expect, it } from "vitest";
import { decodeTicket, normalizeTicket } from "./ticket";

const PEER = "12D3KooWQYhTNQdmr3ArTeUHRYzFg94BKyTkoWBDWez9kSCVe2Xo";

function makeTicket(payload: Record<string, unknown>): string {
  const json = JSON.stringify(payload);
  const b64 = btoa(json).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return `RM1:${b64}`;
}

const VALID = {
  v: 1,
  sid: "abc123-9f",
  name: "Late Night Mix",
  peer: PEER,
  addrs: ["/ip4/192.168.1.7/tcp/54001"],
  auth: true,
};

describe("normalizeTicket", () => {
  it("strips embedded whitespace (line-wrapped codes from chat/email)", () => {
    expect(normalizeTicket("RM1:ab\ncd  ef\tgh\r\nij")).toBe("RM1:abcdefghij");
  });
});

describe("decodeTicket", () => {
  it("decodes a well-formed ticket", () => {
    const info = decodeTicket(makeTicket(VALID));
    expect(info).not.toBeNull();
    expect(info!.sid).toBe("abc123-9f");
    expect(info!.name).toBe("Late Night Mix");
    expect(info!.auth).toBe(true);
    expect(info!.peer).toBe(PEER);
  });

  it("tolerates a line-wrapped paste", () => {
    const t = makeTicket(VALID);
    const wrapped = `${t.slice(0, 20)}\n${t.slice(20, 45)} ${t.slice(45)}`;
    expect(decodeTicket(wrapped)).not.toBeNull();
  });

  it("rejects garbage the extension would refuse (no silent dead ends)", () => {
    expect(decodeTicket("")).toBeNull();
    expect(decodeTicket("RM1:")).toBeNull();
    expect(decodeTicket("RM1:!!!!")).toBeNull();
    expect(decodeTicket("RM2:aGVsbG8")).toBeNull(); // wrong prefix
    expect(decodeTicket(`RM1:${"x".repeat(9000)}`)).toBeNull(); // over the cap
    // base64 that decodes here but uses chars outside base64url (extension rejects)
    expect(decodeTicket("RM1:ab+d")).toBeNull();
    expect(decodeTicket(makeTicket({ ...VALID, v: 2 }))).toBeNull();
    expect(decodeTicket(makeTicket({ ...VALID, sid: "" }))).toBeNull();
    expect(decodeTicket(makeTicket({ ...VALID, peer: "not-a-peer-id" }))).toBeNull();
    expect(decodeTicket(makeTicket({ ...VALID, addrs: [] }))).toBeNull();
    expect(decodeTicket(makeTicket({ ...VALID, addrs: ["not an addr"] }))).toBeNull();
  });
});
