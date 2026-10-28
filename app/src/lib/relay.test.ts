import { describe, expect, it } from "vitest";
import { parseRelayCode, relayHost } from "./relay";

const PEER = "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN";
const KEY = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const code = (q: string) => `reamesh://relay/${PEER}?${q}`;

describe("parseRelayCode", () => {
  it("reads every field of a well-formed code", () => {
    const r = parseRelayCode(
      code(`a=203.0.113.5,2001:db8::1&d=Relay.Example.org&t=4001&q=4002&k=${KEY}&n=Studio%20Relay`),
    );
    expect(r).toMatchObject({
      peer: PEER,
      ips: ["203.0.113.5", "2001:db8::1"],
      dns: "relay.example.org",
      tcp: 4001,
      quic: 4002,
      label: "Studio Relay",
    });
    expect(relayHost(r!)).toBe("relay.example.org");
    expect(relayHost(parseRelayCode(code(`a=203.0.113.5&t=4001&k=${KEY}`))!)).toBe("203.0.113.5");
  });

  it("tolerates a wrapped paste and a capitalised scheme", () => {
    const c = code(`a=203.0.113.5&t=4001&k=${KEY}`);
    expect(parseRelayCode(`  ${c.slice(0, 30)}\n${c.slice(30)} `)?.code).toBe(c);
    expect(parseRelayCode(c.replace("reamesh://relay/", "ReaMesh://Relay/"))).not.toBeNull();
  });

  it("refuses what the extension refuses", () => {
    for (const bad of [
      code(`a=203.0.113.5&t=4001`), // no key
      code(`a=203.0.113.5&t=4001&k=abcd`),
      code(`a=203.0.113.5&t=4001&k=${KEY.toUpperCase()}`),
      code(`a=203.0.113.5&k=${KEY}`), // no port
      code(`t=4001&k=${KEY}`), // nowhere to dial
      code(`a=127.0.0.1&t=4001&k=${KEY}`), // loopback only
      code(`d=bad_name&t=4001&k=${KEY}`),
      code(`d=1.2.3.4&t=4001&k=${KEY}`),
      code(`a=203.0.113.5&t=70000&k=${KEY}`),
      `reamesh://relay/not-a-peer?a=203.0.113.5&t=4001&k=${KEY}`,
      `reamesh://join/${PEER}?a=203.0.113.5&t=4001&k=${KEY}`,
      "",
    ]) {
      expect(parseRelayCode(bad), bad).toBeNull();
    }
  });
});
