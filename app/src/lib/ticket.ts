// Invite-code (ticket) parsing on the app side. What the app reads out of a
// pasted code: enough to show the session name, ask for a passphrase when one
// is needed, and match the extension's `probe` result. The addresses inside
// are the extension's business: the whole normalized code is sent along with
// join/check.
//
// These checks deliberately mirror the extension's `parse_ticket`. Anything
// the app accepts but the extension rejects becomes a dead end for the user: a
// name renders, the passphrase field appears, Join enables, and then nothing
// happens and no error is ever shown.

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
 *  Invite codes travel through chat and email, which line-wrap them. `atob`
 *  tolerates embedded whitespace; the extension's base64url decoder rejects any
 *  character outside the alphabet. So a wrapped code that decodes fine here
 *  would be refused over there: the app would show the session name and enable
 *  Join, and the join would then fail with nothing on screen to explain it.
 *  Normalizing here fixes both halves: the code becomes valid, and what we send
 *  is what we validated. */
export function normalizeTicket(raw: string): string {
  return raw.replace(/\s+/g, "");
}

export function decodeTicket(raw: string): TicketInfo | null {
  const s = normalizeTicket(raw);
  if (!s.startsWith("RM1:") || s.length > 8192) return null;
  const body = s.slice(4);
  if (!/^[A-Za-z0-9_-]+$/.test(body)) return null;
  try {
    const b64 = body.replace(/-/g, "+").replace(/_/g, "/");
    const pad = b64.length % 4 === 2 ? "==" : b64.length % 4 === 3 ? "=" : "";
    const json = JSON.parse(
      new TextDecoder().decode(Uint8Array.from(atob(b64 + pad), (c) => c.charCodeAt(0))),
    );
    if (json?.v !== 1 || typeof json.sid !== "string" || !json.sid) return null;
    // The extension requires `peer` to parse as a PeerId and at least one
    // address to parse as a Multiaddr: it pins the join to that peer, so a
    // ticket without a usable one is not joinable by any path.
    if (typeof json.peer !== "string" || !/^[1-9A-HJ-NP-Za-km-z]{40,}$/.test(json.peer))
      return null;
    if (!Array.isArray(json.addrs) || json.addrs.length === 0) return null;
    if (!json.addrs.some((a: unknown) => typeof a === "string" && a.startsWith("/")))
      return null;
    return {
      sid: json.sid,
      name: typeof json.name === "string" ? json.name : "",
      auth: !!json.auth,
      peer: json.peer,
    };
  } catch {
    return null;
  }
}
