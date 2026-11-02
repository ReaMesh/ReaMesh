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
  /** That track's name; "" when it has none, so the roster says "Track 7". */
  track_name?: string;
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
  /** How this person is connected: "relay" through a relay circuit, "direct"
   *  once a relayed connection was upgraded to a direct one; "" / absent for
   *  the plain LAN or VPN case. */
  link?: "relay" | "direct" | "";
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
  /** Bytes a join would still write: what isn't already in this session's copy. */
  needed?: number;
  /** Bytes free on that disk; absent when it can't be told, like `enough`. */
  free?: number;
  /** `needed` fits in `free` with the download's reserve left over. */
  enough?: boolean;
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
  /** The host build's compatibility number and version (absent from a build
   *  that predates them). A different `proto` can't be joined. */
  proto?: number;
  version?: string;
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
  /** Full paths this REAPER is using: every open project tab, the session's
   *  project, and the folder a join downloads into. Absent from older
   *  extensions. */
  in_use?: string[];
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
  /** How many hosts ended at a cancelled Save As (an untitled project gets one
   *  first). Goes up instead of an error: the user answered the dialog. */
  host_aborted?: number;
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
  /** Outside a progressive join: items waiting for a file a peer just added,
   *  and the bytes of the files on their way (0 until a first piece has said how
   *  big a file is). */
  waiting_media?: number;
  fetch_done?: number;
  fetch_total?: number;
  /** Items that gave up waiting for their file and are not in the project here.
   *  `file` names the first file missing. Retried with `retry_media`; null when
   *  nothing is missing. */
  missing_media?: { count: number; file: string } | null;
  /** Devices this host has blocked (process-global in the extension, so the
   *  list outlives a stop-hosting / re-host cycle). Absent from an older
   *  extension. */
  bans?: BanEntry[];
  /** Recent pings ("look here" at a timeline spot), newest last, kept for a
   *  few seconds. The HUD shows each once; `self` = this user sent it. Absent
   *  from an older extension. */
  pings?: PingEntry[];
  /** Version of the REAPER this extension runs in ("7.42"). Absent from an
   *  older extension. */
  reaper?: string;
  /** This REAPER is older than the extension supports (`min`). The extension
   *  runs as a stub that publishes only this frame; the app shows an "update
   *  REAPER" screen and sends it nothing. */
  unsupported?: { version: string; min: string } | null;
}

/** One ping, as the extension reports it. `id` counts up per REAPER run and
 *  `at` is unix ms; `label` is `pos` in this user's ruler format. */
export interface PingEntry {
  id: number;
  at: number;
  name: string;
  /** Sender's display name and identity color ("" when unknown). */
  by: string;
  color: string;
  pos: number;
  label: string;
  self: boolean;
}

/** One running REAPER instance (= one control server port). */
export interface Instance {
  port: number;
  pid?: number;
  /** The extension build's version and compatibility number, from its hello
   *  (absent from a build that predates them). */
  version?: string;
  proto?: number;
  state?: ExtState;
  /** True while REAPER's main thread isn't ticking (modal dialog open there). */
  busy?: boolean;
  /** Relay reservations while hosting and the latest relay test. */
  relay?: RelayInfo;
}

/** One relay as the hosting run sees it. */
export interface RelayStatus {
  peer: string;
  /** The relay's name for the key, else the code's label ("" for none). */
  label: string;
  host: string;
  state: "connecting" | "reserved" | "retrying" | "refused";
  /** Why the last attempt failed: bad_key, banned, full, not_relay,
   *  unreachable, timeout, lost, refused ("" = it didn't). */
  reason: string;
}

/** Outcome of Settings → Relay → Test, keyed by the request's id. */
export interface RelayTest {
  id: string;
  peer: string;
  /** testing | ok | bad_key | banned | full | not_relay | unreachable |
   *  timeout | invalid | refused */
  outcome: string;
  label?: string;
}

export interface RelayInfo {
  status: RelayStatus[];
  test: RelayTest | null;
}

export function normalizeRelay(msg: Record<string, unknown>): RelayInfo {
  const status = Array.isArray(msg.status) ? (msg.status as RelayStatus[]) : [];
  const test =
    msg.test && typeof msg.test === "object" && typeof (msg.test as RelayTest).id === "string"
      ? (msg.test as RelayTest)
      : null;
  return { status, test };
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
    pings: arr(msg.pings),
  };
}

/** The letter on someone's avatar: the first character of their name. One
 *  letter, because two taken from one name ("DA", "AN") read like an
 *  abbreviation. Taken as a whole code point: indexing with [0] would split an
 *  astral character (an emoji name) into a lone surrogate, which renders as a
 *  replacement glyph in every peer's roster. */
export function initials(name: string): string {
  const first = Array.from(name.trim())[0];
  return first ? first.toUpperCase() : "?";
}
