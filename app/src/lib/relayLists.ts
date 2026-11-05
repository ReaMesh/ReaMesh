// The relays' optional session lists, on the app's side: which relays this
// install's sessions stay off, what the Lobby shows, and how those two read.
//
// The extension reads the lists while a screen holds a lease (`useRelayWatch`)
// and sends them on the `relay` message, one per relay, every entry already
// checked against its signature. The merging below is plain functions, tested
// without any of the wiring.

import { useEffect, useSyncExternalStore } from "react";
import { t } from "@/lib/i18n";
import { relayHost, type RelayCode } from "@/lib/relay";
import { relayAbout } from "@/lib/relays";
import type { RelayList, RelayListEntry, SessionAd } from "@/lib/types";

/** The extension drops a lease it hasn't heard about for a minute. */
const LEASE_RENEW_MS = 20_000;

const HIDDEN_KEY = "reamesh.relayHidden";
const KNOWN_KEY = "reamesh.relayListKnown";

function read<T>(key: string, fallback: T): T {
  try {
    return (JSON.parse(localStorage.getItem(key) ?? "null") as T | null) ?? fallback;
  } catch {
    return fallback;
  }
}

function write(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Private window or full storage: the choice lasts until the app closes.
  }
}

// The user's choice per relay. Listed is the default, so only the exceptions
// are kept: a relay added later starts out listed.
let hidden: string[] = read<string[]>(HIDDEN_KEY, []);
const listeners = new Set<() => void>();

function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

export function subscribeHiddenRelays(cb: () => void): () => void {
  return subscribe(cb);
}

/** For sending: as stored, in case the other window (Settings on macOS) is the
 *  one that ran longer. */
export function getHiddenRelays(): string[] {
  return read<string[]>(HIDDEN_KEY, hidden);
}

export function useHiddenRelays(): string[] {
  return useSyncExternalStore(subscribe, () => hidden);
}

export function setRelayListed(peer: string, listed: boolean) {
  const next = hidden.filter((p) => p !== peer);
  if (!listed) next.push(peer);
  hidden = next;
  write(HIDDEN_KEY, hidden);
  listeners.forEach((l) => l());
}

// What each relay last said about having a list, across runs. A relay that
// can't be reached right now only gets a section for that if it had a list
// when last seen, and the host card knows right away what to offer.
let known: Record<string, boolean> = read<Record<string, boolean>>(KNOWN_KEY, {});

export function noteRelayLists(lists: Record<string, RelayList>) {
  let changed = false;
  for (const [peer, l] of Object.entries(lists)) {
    if (typeof l.enabled === "boolean" && known[peer] !== l.enabled) {
      known = { ...known, [peer]: l.enabled };
      changed = true;
    }
  }
  if (changed) write(KNOWN_KEY, known);
}

export function relayKeepsList(peer: string, list: RelayList | undefined): boolean {
  return typeof list?.enabled === "boolean" ? list.enabled : !!known[peer];
}

/** The name a relay goes by in the app: what its operator calls it, else the
 *  code's label. Keys are often named per person, and "Sessions via lena" reads
 *  as if the list were hers. */
export function relayName(r: RelayCode): string {
  return relayAbout(undefined, r.peer)?.name || r.label || relayHost(r);
}

/** One row of a relay's section. */
export interface RelayRow {
  /** Ready for the same card the local network's sessions use. */
  session: SessionAd;
  /** An invite link naming every relay this session was found on. */
  ticket: string;
  /** The names of those relays, the section's own first. */
  via: string[];
  /** The operator's name for the host's key on the first of them. */
  keyLabel: string;
}

export interface RelaySection {
  relay: RelayCode;
  name: string;
  /** Only what's wrong, or nothing: `down` | `refused` | `expired`. */
  problem: "" | "down" | "refused" | "expired";
  /** The relay stopped relaying this month. */
  budgetUsed: boolean;
  rows: RelayRow[];
}

function toSession(e: RelayListEntry): SessionAd {
  return {
    session_id: e.session_id,
    session_name: e.session_name,
    project_file: "",
    owner: e.owner,
    owner_name: e.owner_name,
    owner_color: e.owner_color,
    auth: e.auth,
    files: e.files,
    bytes: e.bytes,
    proto: e.proto,
    version: e.version,
  };
}

/** The `r=` parameters of a ticket. */
function ticketRelays(ticket: string): string[] {
  return ticket.split("&").filter((p) => p.startsWith("r="));
}

/** A ticket that also names the relays of `other`. Both were built by the
 *  extension from its own relay codes. */
export function mergeTicket(ticket: string, other: string): string {
  const have = new Set(ticketRelays(ticket));
  const more = ticketRelays(other).filter((r) => !have.has(r));
  return more.length ? `${ticket}&${more.join("&")}` : ticket;
}

/** Why a section has something to say instead of sessions. */
function problemOf(list: RelayList | undefined): RelaySection["problem"] {
  if (list?.state === "down") return "down";
  if (list?.state !== "refused") return "";
  return list.reason === "expired" ? "expired" : "refused";
}

/** The Lobby's relay sections: in the order of the settings, one per relay
 *  that has something to show. A relay nobody hosts on shows nothing at all, a
 *  relay in trouble says so. A session stays in one place: the local network
 *  wins over any relay (it's the shorter way), the first relay over a later
 *  one, whose way in is added to the link in case the first one fails. A
 *  second host claiming the same session id is marked on both sides, like two
 *  hosts on one network. */
export function relaySections(
  relays: RelayCode[],
  lists: Record<string, RelayList>,
  lan: SessionAd[],
): { sections: RelaySection[]; contested: Set<string> } {
  const contested = new Set<string>();
  const shown = new Map<string, RelayRow>();
  const sections: RelaySection[] = [];
  for (const relay of relays) {
    const list = lists[relay.peer];
    if (!relayKeepsList(relay.peer, list)) continue;
    const name = relayName(relay);
    const rows: RelayRow[] = [];
    for (const e of list?.entries ?? []) {
      const here = lan.find((s) => s.session_id === e.session_id);
      if (here?.owner === e.owner) continue;
      if (here) contested.add(e.session_id);
      const earlier = shown.get(e.session_id);
      if (earlier) {
        if (earlier.session.owner === e.owner) {
          earlier.ticket = mergeTicket(earlier.ticket, e.ticket);
          earlier.via.push(name);
        } else {
          contested.add(e.session_id);
        }
        continue;
      }
      const row: RelayRow = {
        session: toSession(e),
        ticket: e.ticket,
        via: [name],
        keyLabel: e.key_label,
      };
      shown.set(e.session_id, row);
      rows.push(row);
    }
    const problem = problemOf(list);
    if (rows.length > 0 || problem) {
      sections.push({ relay, name, problem, budgetUsed: !!list?.budget_used, rows });
    }
  }
  for (const s of sections) {
    for (const r of s.rows) {
      if (contested.has(r.session.session_id)) r.session.contested = true;
    }
  }
  return { sections, contested };
}

/** The relays a host can choose between: the ones that keep a list. */
export function listRelays(relays: RelayCode[], lists: Record<string, RelayList>): RelayCode[] {
  return relays.filter((r) => relayKeepsList(r.peer, lists[r.peer]));
}

/** "Visible on Band relay and Studio": the line under the project name. */
export function hostVisibleText(relays: RelayCode[], hiddenPeers: string[]): string {
  const names = relays.filter((r) => !hiddenPeers.includes(r.peer)).map(relayName);
  if (names.length === 0) return t("hostVisibleNone");
  if (names.length === 1) return t("hostVisibleOne", { name: names[0] });
  if (names.length === 2) return t("hostVisibleTwo", { a: names[0], b: names[1] });
  return t("hostVisibleMany", { list: names.slice(0, -1).join(", "), last: names[names.length - 1] });
}

/** What that line means, for its tooltip. */
export function hostListNote(relays: RelayCode[], hiddenPeers: string[], passworded: boolean): string {
  if (relays.every((r) => hiddenPeers.includes(r.peer))) return t("hostListOff");
  return t(passworded ? "hostListProtected" : "hostListOpen");
}

/** "Band relay and Studio", for the join dialog. */
export function viaNames(via: string[]): string {
  if (via.length < 2) return via[0] ?? "";
  return t("hostViaMany", { list: via.slice(0, -1).join(", "), last: via[via.length - 1] });
}

/** Holds a lease on the lists while `active`. `send` names the extension. */
export function useRelayWatch(
  id: string,
  active: boolean,
  port: number | null,
  send: (cmd: Record<string, unknown>, port?: number) => void,
) {
  useEffect(() => {
    if (!active || port === null) return;
    const renew = () => send({ cmd: "relay_watch", id, on: true }, port);
    renew();
    const timer = setInterval(renew, LEASE_RENEW_MS);
    return () => {
      clearInterval(timer);
      send({ cmd: "relay_watch", id, on: false }, port);
    };
  }, [id, active, port, send]);
}
