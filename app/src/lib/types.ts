// Shapes of the JSON the extension's control server sends. One `state`
// message mirrors the whole extension state; the app holds no other truth.

import { t } from "./i18n";

export type Mode = "idle" | "browsing" | "downloading" | "hosting" | "client";

export interface Peer {
  id: string;
  /** Display name from the peer's presence beacon; "" if they never sent one. */
  name: string;
  /** Identity color (hex). Always present: derived from the peer id if unset. */
  color: string;
  is_self: boolean;
  is_owner: boolean;
  /** 1-based number of the track this peer currently has focused, resolved
   *  locally from their broadcast track GUID. Absent/0 when they're not on a
   *  track (or an older extension that doesn't report focus). */
  track?: number | null;
  /** Peer's playhead in project seconds (edit cursor, or play position while
   *  rolling). For peer playheads in the arrange overlay. */
  play?: number;
  /** True while the peer's transport is rolling (their playhead is sweeping). */
  playing?: boolean;
  /** GUIDs of the tracks / items the peer has selected, drawn as tinted
   *  "ghosts" in the arrange overlay. */
  seltr?: string[];
  seli?: string[];
}

export interface SessionAd {
  session_id: string;
  session_name: string;
  project_file: string;
  owner: string;
  auth: boolean;
}

/** Structured activity-log entry / error from the extension. `code` (plus
 *  `params`) names the event so the app can render it in the user's language;
 *  `text` is the English fallback for unknown codes (and the whole message
 *  when `code` is empty). */
export interface LogEntry {
  code: string;
  params?: Record<string, string | number>;
  text: string;
}

export interface ExtState {
  mode: Mode;
  /** Rust-side session state: hosting|browsing|connecting|downloading|ready|failed|ended */
  state: string;
  progress: number;
  session: string;
  /** Name (stem) of the project currently open in this REAPER instance. */
  project: string;
  /** Last session-ending event; null when there is none. */
  error: LogEntry | null;
  log: LogEntry[];
  /** Client only: cumulative count of own edits the host overrode. */
  overridden?: number;
  /** True while the extension is trying to rejoin a lost host automatically. */
  reconnecting?: boolean;
  sessions: SessionAd[];
  peers: Peer[];
}

/** One running REAPER instance (= one control server port). */
export interface Instance {
  port: number;
  pid?: number;
  state?: ExtState;
}

export function displayName(p: Peer): string {
  if (p.name) return p.name;
  return t("guest");
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}
