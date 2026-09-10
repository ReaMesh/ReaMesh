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

/** A port parameter: `null` when the value is not a usable port. */
function parsePort(v: string): number | null {
  if (!/^\d{1,5}$/.test(v)) return null;
  const n = Number(v);
  return n >= 1 && n <= 65535 ? n : null;
}

export function decodeTicket(raw: string): TicketInfo | null {
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
  let ips = 0;
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
        ips = v.split(",").filter(isIp).slice(0, MAX_IPS).length;
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
  if (ips === 0 || (tcp === null && quic === null)) return null;
  return { sid, name, auth, peer };
}
