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
  tints_healed: "logTintsHealed",
  media_copied: "logMediaCopied",
  media_copy_failed: "logMediaCopyFailed",
  media_timeout: "logMediaTimeout",
  reconnecting: "logReconnecting",
  reconnect_failed: "reconnectFailed",
  bad_ticket: "logBadTicket",
  bad_changeset: "logBadChangeset",
  ticket_unreachable: "ticketUnreachable",
  version_mismatch: "logVersionMismatch",
  manifest_timeout: "logManifestTimeout",
  download_stalled: "logDownloadStalled",
  host_unreachable: "logHostUnreachable",
  edit_locked: "editLocked",
  peer_tempo_changed: "peerTempoChanged",
  peer_markers_changed: "peerMarkersChanged",
  peer_title_changed: "peerTitleChanged",
  peer_notes_changed: "peerNotesChanged",
  marker_shared: "logMarkerShared",
  follow_ended: "followEnded",
  nofx_open: "nofxOpen",
  verify_hold: "verifyHold",
  reaper_mismatch: "logReaperMismatch",
  progressive_open: "logProgressiveOpen",
  loading_readonly: "logLoadingReadonly",
  media_downloaded: "logMediaDownloaded",
  media_ready: "logMediaReady",
  placeholders_unresolved: "logPlaceholdersUnresolved",
  placeholder_blocked: "logPlaceholderBlocked",
  kicked: "kickedByHost",
  banned: "bannedByHost",
};

export function localizeLogEntry(entry: LogEntry | string): string {
  if (typeof entry === "string") return entry; // older extension: raw text
  // A few codes pick a different string depending on whether the extension
  // could name the thing (track / holder names can be empty).
  switch (entry.code) {
    case "edit_locked": {
      const track = entry.params?.track;
      const by = entry.params?.by;
      if (by && track) return t("editLockedByTrack", { by, track });
      if (by) return t("editLockedBy", { by });
      if (track) return t("editLockedTrack", { track });
      return t("editLocked", entry.params);
    }
    case "peer_track_added": {
      const name = entry.params?.name;
      return name ? t("peerTrackAdded", { name }) : t("peerTrackAddedUnnamed");
    }
    case "peer_track_removed": {
      const name = entry.params?.name;
      return name ? t("peerTrackRemoved", { name }) : t("peerTrackRemovedUnnamed");
    }
    case "marker_received": {
      const name = entry.params?.name ?? "";
      const by = entry.params?.by;
      return by
        ? t("logMarkerReceived", { by, name })
        : t("logMarkerReceivedUnnamed", { name });
    }
    case "plugin_added": {
      const plugin = entry.params?.plugin ?? "";
      const track = entry.params?.track;
      const by = entry.params?.by;
      if (by && track) return t("pluginAddedByTrack", { by, plugin, track });
      if (by) return t("pluginAddedBy", { by, plugin });
      if (track) return t("pluginAddedTrack", { plugin, track });
      return t("pluginAdded", { plugin });
    }
    case "plugins_added": {
      const count = entry.params?.count ?? 0;
      const track = entry.params?.track;
      return track ? t("pluginsAddedTrack", { count, track }) : t("pluginsAdded", { count });
    }
    // Host-side moderation: the guest may never have sent a display name.
    case "peer_kicked": {
      const name = entry.params?.name;
      return name ? t("peerKicked", { name }) : t("peerKickedUnnamed");
    }
    case "peer_banned": {
      const name = entry.params?.name;
      return name ? t("peerBanned", { name }) : t("peerBannedUnnamed");
    }
    case "peer_unbanned": {
      const name = entry.params?.name;
      return name ? t("peerUnbanned", { name }) : t("peerUnbannedUnnamed");
    }
  }
  const key = entry.code ? CODE_KEYS[entry.code] : undefined;
  return key ? t(key, entry.params) : entry.text;
}
