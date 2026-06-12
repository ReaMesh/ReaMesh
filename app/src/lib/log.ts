// Renders the extension's structured activity-log entries ({code, params,
// text}) in the user's language. Codes the app doesn't know fall back to the
// English `text` the extension always sends: an older/newer extension can
// add events without breaking the app.

import { t, type MsgKey } from "./i18n";
import type { LogEntry } from "./types";

const CODE_KEYS: Partial<Record<string, MsgKey>> = {
  browsing: "logBrowsing",
  hosting: "logHosting",
  joining: "logJoining",
  join_unavailable: "logJoinUnavailable",
  join_failed: "logJoinFailed",
  wrong_password: "wrongPassword",
  download_complete: "logDownloadComplete",
  connected: "logConnected",
  peer_joined: "logPeerJoined",
  peer_left: "logPeerLeft",
  left: "logLeft",
  project_changed: "logProjectChanged",
  host_ended: "hostEndedSession",
  host_lost: "hostLost",
  save_first: "logSaveFirst",
  host_failed: "logHostFailed",
  browse_failed: "logBrowseFailed",
  waiting_media: "logWaitingMedia",
  media_applied: "logMediaApplied",
  media_copied: "logMediaCopied",
  media_copy_failed: "logMediaCopyFailed",
  media_timeout: "logMediaTimeout",
  reconnecting: "logReconnecting",
  reconnect_failed: "reconnectFailed",
  edit_overridden: "editOverridden",
};

export function localizeLogEntry(entry: LogEntry | string): string {
  if (typeof entry === "string") return entry; // older extension: raw text
  const key = entry.code ? CODE_KEYS[entry.code] : undefined;
  return key ? t(key, entry.params) : entry.text;
}
