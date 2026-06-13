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
  /** Peer's play cursor in project seconds (sweeps while `playing`). Drawn as a
   *  sweeping playhead in the arrange overlay. */
  play?: number;
  /** True while the peer's transport is rolling (their playhead is sweeping). */
  playing?: boolean;
  /** Peer's edit cursor in project seconds, drawn as a static caret, distinct
   *  from the play cursor. */
  cursor?: number;
  /** Peer's time selection / loop range in project seconds, drawn as a
   *  translucent band across the tracks. Equal values = no selection. */
  tsa?: number;
  tsb?: number;
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

/** Live arrange-view geometry for the peer-playhead / selection-ghost overlay,
 *  in REAPER SCREEN coordinates (logical points). The arrange overlay window
 *  covers REAPER's whole window, so it converts these to window-relative by
 *  subtracting its own screen origin. `{}` (no `tracks`) means nothing to draw. */
export interface ArrangeView {
  /** Screen x of the arrange's left edge (right edge of the track panels). */
  ax: number;
  /** Project time at the arrange's left edge, and seconds per horizontal pixel. */
  t0: number;
  spp: number;
  /** Screen y of the top / bottom of the visible track stack (playhead span). */
  top: number;
  bottom: number;
  /** Per-track screen rect (reflects vertical scroll), keyed by track GUID. */
  tracks?: Record<string, { y: number; h: number }>;
  /** Geometry of every item some peer has selected, keyed by item GUID. */
  items?: Record<string, { tr: string; pos: number; len: number }>;
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
  /** Cumulative count of own edits that snapped back because another peer held
   *  the field's editing lock (either role: the lock binds the host too). */
  overridden?: number;
  /** True while the extension is trying to rejoin a lost host automatically. */
  reconnecting?: boolean;
  sessions: SessionAd[];
  peers: Peer[];
  /** Local arrange-view geometry for the playhead/ghost overlay (may be `{}`). */
  view?: ArrangeView;
  /** Result of a passphrase pre-check: `ok` is null while in flight, then a
   *  boolean. `null` (the whole field) when no check is active. */
  probe?: { session_id: string; ok: boolean | null } | null;
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
