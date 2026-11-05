// Relay codes, as the operator of a self-hosted `reamesh-relay` hands them out.
// Mirrors `parse_relay_code` (relay-proto/src/lib.rs), so the app never
// stores a code the extension would refuse.
//
//   reamesh://relay/<relay peer id>?a=<ip>,<ip>&d=<dns name>&t=<tcp>&q=<quic>
//     &k=<64 hex access key>&n=<percent-encoded label>[&l=1]
//
// `l=1`: the relay kept a session list when it printed the code.
//
// The key stays on this machine: it is sent to the local extension (which
// proves it to the relay while hosting) and never goes into an invite link.

import { isDialableIp, isDnsName, isIp, parsePort } from "@/lib/ticket";

export const RELAY_PREFIX = "reamesh://relay/";
const RELAY_MAX_LEN = 1024;
const MAX_IPS = 8;
const LABEL_MAX = 64;
/** As many relays as the extension takes (its MAX_RELAYS). */
export const MAX_RELAYS = 3;

export interface RelayCode {
  /** The code as pasted (trimmed, whitespace removed): what gets stored and sent. */
  code: string;
  peer: string;
  ips: string[];
  dns: string;
  tcp: number;
  quic: number;
  /** The operator's name for this relay ("" when the code has none). */
  label: string;
  /** The relay kept a session list when it printed this code. Only a hint, for
   *  what to say before the relay is added and may be dialed. */
  list: boolean;
}

export function isRelayCode(s: string): boolean {
  return s.slice(0, RELAY_PREFIX.length).toLowerCase() === RELAY_PREFIX;
}

export function parseRelayCode(raw: string): RelayCode | null {
  const code = raw.replace(/\s+/g, "");
  if (code.length > RELAY_MAX_LEN || !isRelayCode(code)) return null;
  const rest = code.slice(RELAY_PREFIX.length);
  const q = rest.indexOf("?");
  const peer = (q < 0 ? rest : rest.slice(0, q)).replace(/\/+$/, "");
  if (!/^[1-9A-HJ-NP-Za-km-z]{40,}$/.test(peer)) return null;
  let ips: string[] = [];
  let dns = "";
  let tcp = 0;
  let quic = 0;
  let key = false;
  let label = "";
  let list = false;
  for (const pair of (q < 0 ? "" : rest.slice(q + 1)).split("&")) {
    if (!pair) continue;
    const eq = pair.indexOf("=");
    const k = eq < 0 ? pair : pair.slice(0, eq);
    const v = eq < 0 ? "" : pair.slice(eq + 1);
    switch (k) {
      case "a":
        ips = v.split(",").filter(isIp).filter(isDialableIp).slice(0, MAX_IPS);
        break;
      case "d":
        if (v === "") break;
        if (!isDnsName(v)) return null;
        dns = v.toLowerCase();
        break;
      case "t": {
        const p = parsePort(v);
        if (p === null) return null;
        tcp = p;
        break;
      }
      case "q": {
        const p = parsePort(v);
        if (p === null) return null;
        quic = p;
        break;
      }
      case "k":
        if (!/^[0-9a-f]{64}$/.test(v)) return null;
        key = true;
        break;
      case "n":
        try {
          label = decodeURIComponent(v);
        } catch {
          label = "";
        }
        label = [...label].slice(0, LABEL_MAX).join("");
        break;
      case "l":
        list = v === "1";
        break;
      default:
        break;
    }
  }
  if (!key || (ips.length === 0 && !dns) || (tcp === 0 && quic === 0)) return null;
  return { code, peer, ips, dns, tcp, quic, label, list };
}

/** Where the relay is, for display: its DNS name, else its first IP. */
export function relayHost(r: RelayCode): string {
  return r.dns || r.ips[0] || "";
}
