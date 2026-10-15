// Invite-link (ticket) parsing on the app side. What the app reads out of a
// pasted or opened `reamesh://` link: enough to show the session name, ask for
// a passphrase when one is needed, and match the extension's `probe` result.
// The addresses inside are the extension's business: the whole normalized
// link is sent along with join/check.
//
// Format (the extension's `parse_ticket` is the authority):
//
//   reamesh://join/<sid>?n=<name>&s=1&p=<peer>&a=<ip>,<ip>&t=<tcp>&q=<quic>
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

function isIp(s: string): boolean {
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

/** Whether a ticket address can name a host: loopback, unspecified,
 *  link-local, multicast and broadcast never do. Mirrors the extension's
 *  `dialable_ip`, which drops the same addresses from a ticket. */
export function isDialableIp(s: string): boolean {
  if (IPV4.test(s)) return ipv4Dialable(s.split(".").map(Number));
  const g = ipv6Groups(s);
  if (!g) return false;
  // IPv4-mapped (::ffff:a.b.c.d) follows the IPv4 rules.
  if (g.slice(0, 5).every((x) => x === 0) && g[5] === 0xffff) {
    return ipv4Dialable([g[6] >> 8, g[6] & 0xff, g[7] >> 8, g[7] & 0xff]);
  }
  if (g.every((x) => x === 0)) return false; // ::
  if (g.slice(0, 7).every((x) => x === 0) && g[7] === 1) return false; // ::1
  if ((g[0] & 0xffc0) === 0xfe80) return false; // link-local
  if ((g[0] & 0xff00) === 0xff00) return false; // multicast
  return true;
}

/** A port parameter: `null` when the value is not a usable port. */
function parsePort(v: string): number | null {
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
      default:
        break; // a future version's extras are ignored, not refused
    }
  }
  // The extension pins the join to `p` (it must parse as a PeerId) and needs
  // at least one address plus one port to dial.
  if (!/^[1-9A-HJ-NP-Za-km-z]{40,}$/.test(peer)) return null;
  if (ips.length === 0 || (tcp === null && quic === null)) return null;
  return { info: { sid, name, auth, peer }, addrs: { ips, tcp: tcp ?? 0, quic: quic ?? 0 } };
}
