// Invite-link (ticket) parsing on the app side. What the app reads out of a
// pasted or opened `reamesh://` link: enough to show the session name, ask for
// a passphrase when one is needed, and match the extension's `probe` result.
// The addresses inside are the extension's business: the whole normalized
// link is sent along with join/check.
//
// Format (the extension's `parse_ticket` is the authority):
//
//   reamesh://join/<sid>?n=<name>&s=1&p=<peer>&a=<ip>,<ip>&t=<tcp>&q=<quic>
//     &x=<link secret>&r=<relay>~<ip>,<ip>~<tcp>~<quic>[~<dns>]
//
// `x` is an open session's link secret when its host has a relay (32 hex);
// each `r` is one relay the host reserves on (no key, see lib/relay.ts). A
// link may name only relays and no direct address.
//
// These checks deliberately mirror the extension's parser. Anything the app
// accepts but the extension rejects becomes a dead end for the user: a name
// renders, the passphrase field appears, Join enables, and then nothing
// happens and no error is ever shown.

export const TICKET_PREFIX = "reamesh://join/";
const TICKET_MAX_LEN = 8192;
const MAX_IPS = 8;

export interface TicketInfo {
  sid: string;
  name: string;
  auth: boolean;
  /** The host's peer id: the identity the join is pinned to, and the key for
   *  the "you've joined this host before" memory. */
  peer: string;
  /** How many relays the link names (0 = direct addresses only). */
  relays: number;
}

/** Strip every whitespace character, not just the ends.
 *
 *  Invite links travel through chat and email, which line-wrap long ones. The
 *  link itself never contains whitespace (the name is percent-encoded), so
 *  removing all of it restores exactly what the host generated. */
export function normalizeTicket(raw: string): string {
  return raw.replace(/\s+/g, "");
}

/** Does this look like one of our links at all (scheme + path)? Used to pick
 *  an invite out of whatever URLs the OS hands the app. */
export function isInviteLink(s: string): boolean {
  return s.slice(0, TICKET_PREFIX.length).toLowerCase() === TICKET_PREFIX;
}

const IPV4 = /^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/;
// Loose IPv6 shape. The extension is the strict parser; a link the host
// generated always satisfies both, and a hand-mangled one fails either way.
const IPV6 = /^[0-9a-fA-F:]*:[0-9a-fA-F:.]*$/;

export function isIp(s: string): boolean {
  return IPV4.test(s) || (s.length <= 45 && IPV6.test(s));
}

/** The eight 16-bit groups of an IPv6 address (with an optional dotted IPv4
 *  tail), or null when it doesn't parse. */
function ipv6Groups(s: string): number[] | null {
  let text = s;
  const lastColon = text.lastIndexOf(":");
  const v4 = text.slice(lastColon + 1);
  if (v4.includes(".")) {
    // A dotted IPv4 tail is the last two groups.
    if (!IPV4.test(v4)) return null;
    const o = v4.split(".").map(Number);
    const hex = (n: number) => n.toString(16);
    text = `${text.slice(0, lastColon + 1)}${hex((o[0] << 8) | o[1])}:${hex((o[2] << 8) | o[3])}`;
  }
  const halves = text.split("::");
  if (halves.length > 2) return null;
  const part = (h: string) => (h === "" ? [] : h.split(":"));
  const head = part(halves[0]);
  const tail = halves.length === 2 ? part(halves[1]) : [];
  const all = [...head, ...tail];
  if (all.some((g) => !/^[0-9a-fA-F]{1,4}$/.test(g))) return null;
  // Without "::" there must be exactly eight groups; with it, fewer.
  if (halves.length === 1 ? all.length !== 8 : all.length > 7) return null;
  const fill = Array<string>(8 - all.length).fill("0");
  return [...head, ...fill, ...tail].map((g) => parseInt(g, 16));
}

function ipv4Dialable(o: number[]): boolean {
  const [a, b] = o;
  if (a === 127) return false; // loopback
  if (o.every((x) => x === 0)) return false; // unspecified
  if (a === 169 && b === 254) return false; // link-local
  if (a >= 224 && a <= 239) return false; // multicast
  if (o.every((x) => x === 255)) return false; // broadcast
  return true;
}

/** The IPv4 octets of an IPv4-mapped address (::ffff:a.b.c.d), else null. */
function mappedIpv4(g: number[]): number[] | null {
  if (!g.slice(0, 5).every((x) => x === 0) || g[5] !== 0xffff) return null;
  return [g[6] >> 8, g[6] & 0xff, g[7] >> 8, g[7] & 0xff];
}

/** Whether a ticket address can name a host: loopback, unspecified,
 *  link-local, multicast and broadcast never do. Mirrors the extension's
 *  `dialable_ip`, which drops the same addresses from a ticket. */
export function isDialableIp(s: string): boolean {
  if (IPV4.test(s)) return ipv4Dialable(s.split(".").map(Number));
  const g = ipv6Groups(s);
  if (!g) return false;
  // IPv4-mapped follows the IPv4 rules.
  const mapped = mappedIpv4(g);
  if (mapped) return ipv4Dialable(mapped);
  if (g.every((x) => x === 0)) return false; // ::
  if (g.slice(0, 7).every((x) => x === 0) && g[7] === 1) return false; // ::1
  if ((g[0] & 0xffc0) === 0xfe80) return false; // link-local
  if ((g[0] & 0xff00) === 0xff00) return false; // multicast
  return true;
}

/** A port parameter: `null` when the value is not a usable port. */
export function parsePort(v: string): number | null {
  if (!/^\d{1,5}$/.test(v)) return null;
  const n = Number(v);
  return n >= 1 && n <= 65535 ? n : null;
}

/** Where an invite link says its host can be dialed. */
export interface TicketAddrs {
  ips: string[];
  /** 0 when the link names no such port. */
  tcp: number;
  quic: number;
}

/** Who an invite link's addresses can reach. */
export type TicketReach = "lan" | "lan_tailscale" | "tailscale" | "public";

function ipReach(ip: string): "lan" | "tailscale" | "public" {
  const g = IPV4.test(ip) ? null : ipv6Groups(ip);
  const o = g ? mappedIpv4(g) : IPV4.test(ip) ? ip.split(".").map(Number) : null;
  // Home routers firewall inbound IPv6, so even a global address promises
  // nothing beyond the local network.
  if (!o) return "lan";
  const [a, b, c] = o;
  if (a === 100 && b >= 64 && b <= 127) return "tailscale"; // 100.64.0.0/10
  const local =
    a === 10 ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) ||
    // Never routed on the internet: this network, loopback, link-local, the
    // documentation and benchmark ranges, multicast and reserved.
    a === 0 ||
    a === 127 ||
    (a === 169 && b === 254) ||
    (a === 192 && b === 0 && (c === 0 || c === 2)) ||
    (a === 198 && (b === 18 || b === 19 || (b === 51 && c === 100))) ||
    (a === 203 && b === 0 && c === 113) ||
    a >= 224;
  return local ? "lan" : "public";
}

/** How far a host's addresses carry an invite: a public IPv4 address reaches
 *  the internet, 100.64.0.0/10 other Tailscale devices, the rest (RFC 1918, ULA
 *  and global IPv6) the local network. */
export function ticketReach(ips: string[]): TicketReach {
  const kinds = new Set(ips.map(ipReach));
  if (kinds.has("public")) return "public";
  if (!kinds.has("tailscale")) return "lan";
  return kinds.has("lan") ? "lan_tailscale" : "tailscale";
}

export function decodeTicket(raw: string): TicketInfo | null {
  return parseTicket(raw)?.info ?? null;
}

/** The host's addresses from its own invite link: what it advertises to
 *  nearby devices over Bluetooth (lib/nearby.ts). */
export function ticketAddrs(raw: string): TicketAddrs | null {
  return parseTicket(raw)?.addrs ?? null;
}

function parseTicket(raw: string): { info: TicketInfo; addrs: TicketAddrs } | null {
  const s = normalizeTicket(raw);
  if (s.length > TICKET_MAX_LEN || !isInviteLink(s)) return null;
  const rest = s.slice(TICKET_PREFIX.length);
  const q = rest.indexOf("?");
  const sid = (q < 0 ? rest : rest.slice(0, q)).replace(/\/+$/, "");
  if (!/^[A-Za-z0-9-]{1,64}$/.test(sid)) return null;
  const query = q < 0 ? "" : rest.slice(q + 1);

  let name = "";
  let auth = false;
  let peer = "";
  let ips: string[] = [];
  let tcp: number | null = null;
  let quic: number | null = null;
  const relays = new Set<string>();
  for (const pair of query.split("&")) {
    if (!pair) continue;
    const eq = pair.indexOf("=");
    const k = eq < 0 ? pair : pair.slice(0, eq);
    const v = eq < 0 ? "" : pair.slice(eq + 1);
    switch (k) {
      case "n":
        // A garbled name is cosmetic: show nothing rather than refuse.
        try {
          name = decodeURIComponent(v);
        } catch {
          name = "";
        }
        break;
      case "s":
        auth = v === "1";
        break;
      case "p":
        peer = v;
        break;
      case "a":
        ips = v.split(",").filter(isIp).filter(isDialableIp).slice(0, MAX_IPS);
        break;
      case "t":
        tcp = parsePort(v);
        if (tcp === null) return null;
        break;
      case "q":
        quic = parsePort(v);
        if (quic === null) return null;
        break;
      case "x":
        if (!/^[0-9a-f]{32}$/.test(v)) return null;
        break;
      case "r": {
        // A relay entry that doesn't parse is skipped, like the extension does.
        const peerOf = parseTicketRelay(v);
        if (peerOf && relays.size < MAX_RELAYS) relays.add(peerOf);
        break;
      }
      default:
        break; // a future version's extras are ignored, not refused
    }
  }
  // The extension pins the join to `p` (it must parse as a PeerId) and needs
  // at least one address plus one port to dial.
  if (!PEER_ID.test(peer)) return null;
  const direct = ips.length > 0 && (tcp !== null || quic !== null);
  if (!direct && relays.size === 0) return null;
  return {
    info: { sid, name, auth, peer, relays: relays.size },
    addrs: { ips: direct ? ips : [], tcp: direct ? (tcp ?? 0) : 0, quic: direct ? (quic ?? 0) : 0 },
  };
}

const PEER_ID = /^[1-9A-HJ-NP-Za-km-z]{40,}$/;
const MAX_RELAYS = 3;

/** A hostname a relay may be named by (the extension's `valid_dns_name`). */
export function isDnsName(d: string): boolean {
  if (d.length === 0 || d.length > 253 || isIp(d)) return false;
  return d
    .split(".")
    .every((l) => l.length >= 1 && l.length <= 63 && /^[A-Za-z0-9-]+$/.test(l) && !l.startsWith("-") && !l.endsWith("-"));
}

/** One `r=` entry (`<peer>~<ips>~<tcp>~<quic>[~<dns>]`): its relay peer id,
 *  or null when the extension's `RelayAddr::from_ticket_param` would refuse it. */
function parseTicketRelay(v: string): string | null {
  const parts = v.split("~");
  if (parts.length < 4 || parts.length > 5) return null;
  const [peer, ipList, t, q, dns] = parts;
  if (!PEER_ID.test(peer)) return null;
  const ips = ipList.split(",").filter(isIp).filter(isDialableIp);
  const port = (s: string) => (s === "" ? 0 : parsePort(s));
  const tp = port(t);
  const qp = port(q);
  if (tp === null || qp === null) return null;
  if (dns !== undefined && !isDnsName(dns)) return null;
  if (ips.length === 0 && !dns) return null;
  if (tp === 0 && qp === 0) return null;
  return peer;
}
