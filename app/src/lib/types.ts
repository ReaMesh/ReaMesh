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
  /** Peer id this peer is following ("" / absent = nobody). On the self entry
   *  it is this user's own follow target; on others it powers the "following
   *  you" indicator. */
  follow?: string;
  /** This peer's REAPER main thread stopped responding (their network thread
   *  still beacons, so we can tell the difference from them leaving). */
  frozen?: boolean;
  /** Smoothed ping round trip to this peer in ms (0 / absent = not measured,
   *  and always 0 on the self entry). Drives the "slow connection" notice. */
  rtt?: number;
}

/** Join preview from the owner's manifest. Sizes and metadata are host-reported;
 *  the extension checks the plugin list against the received project before opening it. */
export interface SessionPreview {
  files?: number;
  bytes?: number;
  project_file?: string;
  /** `reaper` is the host's REAPER version ("7.42"; absent from an older
   *  host): the trust dialog warns when it differs from the local one. */
  meta?: { v?: number; tracks?: number; plugins?: string[]; reaper?: string } | null;
}

export interface SessionAd {
  session_id: string;
  session_name: string;
  project_file: string;
  owner: string;
  /** The host's *self-reported* display name/color from the announce. There is
   *  no presence beacon before a join, so this is the only pre-join signal of
   *  who is hosting, and it is chosen by the host, so present it as a claimed
   *  label, never as a verified identity. "" from an older host. */
  owner_name?: string;
  owner_color?: string;
  auth: boolean;
  /** Shareable file count + total bytes, from the announce (0 = unknown/old host). */
  files?: number;
  bytes?: number;
  /** A second peer announced this same session id. The first-seen owner keeps
   *  the entry, but someone on the network is impersonating the session, so the
   *  lobby warns before a join. */
  contested?: boolean;
}

/** One entry of the host's ban table. `expires_at` is unix ms (absolute, so
 *  the state frame doesn't churn every tick); the app computes "N min left". */
export interface BanEntry {
  id: string;
  /** Display name the peer had when it was blocked ("" if it never sent one). */
  name: string;
  /** Identity color (hex) at block time, so the avatar matches the roster's. */
  color: string;
  expires_at: number;
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
  /** Byte counters behind `progress` while downloading (0 or absent otherwise). */
  done_bytes?: number;
  total_bytes?: number;
  /** File counters for the same download. */
  files_done?: number;
  files_total?: number;
  /** Relative path of the file currently being fetched ("" outside a download). */
  current_file?: string;
  /** True while REAPER is blocking in Main_openProject on the finished download. */
  opening?: boolean;
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
  /** Invite code for the hosted session ("" while the host's addresses are
   *  still being learned, and always "" when not hosting). Someone outside
   *  mDNS reach (VPN, strict LAN) pastes it in their Lobby to join. */
  ticket?: string;
  sessions: SessionAd[];
  peers: Peer[];
  /** Result of a passphrase pre-check: `ok` is null while in flight, then a
   *  boolean. `null` (the whole field) when no check is active. A successful
   *  check also carries the join `preview`. */
  /** `reason` says why `ok` is false: "rejected" is the only wrong-passphrase
   *  case; "banned" means the host blocked this device; "timeout" /
   *  "unreachable" / "gone" mean the host didn't answer. */
  probe?: {
    session_id: string;
    ok: boolean | null;
    preview?: SessionPreview | null;
    reason?: string;
  } | null;
  /** Pre-host warning: hosting would copy this much external media into the
   *  project folder (and/or the project lives in a cloud-synced folder). The
   *  app confirms before re-sending `host` with confirm:true. */
  host_warning?: {
    files: number;
    bytes: number;
    free: number;
    free_known: boolean;
    cloud: boolean;
    enough: boolean;
  } | null;
  /** Post-download verification: the received project loads plugins the host
   *  didn't list (`listed` false = the host reported no list at all). The open
   *  is held until `open_confirm` or `leave`. */
  verify?: { extras: string[]; listed: boolean } | null;
  /** This client joined without effects (view-only, every FX chain stripped). */
  nofx?: boolean;
  /** Progressive join: the project is open but still being received (media
   *  downloading in the background and/or placeholder items waiting for it).
   *  Edits snap back meanwhile; the download counters above keep moving. */
  loading?: boolean;
  /** Items currently shown as empty placeholders because their media hasn't
   *  arrived yet (progressive join). */
  placeholders?: number;
  /** Devices this host has blocked (process-global in the extension, so the
   *  list outlives a stop-hosting / re-host cycle). Absent from an older
   *  extension. */
  bans?: BanEntry[];
  /** Version of the REAPER this extension runs in ("7.42"). Absent from an
   *  older extension. */
  reaper?: string;
  /** This REAPER is older than the extension supports (`min`). The extension
   *  runs as a stub that publishes only this frame; the app shows an "update
   *  REAPER" screen and sends it nothing. */
  unsupported?: { version: string; min: string } | null;
}

/** One running REAPER instance (= one control server port). */
export interface Instance {
  port: number;
  pid?: number;
  state?: ExtState;
  /** True while REAPER's main thread isn't ticking (modal dialog open there). */
  busy?: boolean;
}

export function displayName(p: Peer): string {
  if (p.name) return p.name;
  return t("guest");
}

/** Normalize array fields at the process boundary. The extension versions
 *  independently of the app, so missing or malformed arrays must not reach
 *  components that call map or filter without further checks. */
export function normalizeState(msg: Record<string, unknown>): ExtState {
  const arr = <T,>(v: unknown): T[] => (Array.isArray(v) ? (v as T[]) : []);
  return {
    ...(msg as unknown as ExtState),
    peers: arr(msg.peers),
    sessions: arr(msg.sessions),
    log: arr(msg.log),
    bans: arr(msg.bans),
  };
}

/** First character of a word, as a whole code point: indexing with [0] would
 *  split an astral character (an emoji name) into a lone surrogate, which
 *  renders as a replacement glyph in every peer's roster. */
function firstChar(word: string): string {
  return Array.from(word)[0] ?? "";
}

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return Array.from(parts[0]).slice(0, 2).join("").toUpperCase();
  return (firstChar(parts[0]) + firstChar(parts[parts.length - 1])).toUpperCase();
}
