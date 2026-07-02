// Tiny i18n layer: English + German, no library. The preference
// ("system" | "en" | "de") is persisted backend-side (like the REAPER folder)
// so the native macOS settings window and this webview share one source of
// truth, and mirrored into localStorage for a synchronous first render.
// "system" resolves through the device language; anything that isn't German
// falls back to English.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Lang = "en" | "de";
export type LangPref = "system" | Lang;

const KEY = "reamesh.lang";

const en = {
  // Disconnected
  waitingForReaper: "Waiting for REAPER",
  startReaperToConnect: "Start REAPER to connect.",
  openReaper: "Open REAPER",
  couldntOpenReaper: "Couldn't open REAPER. Launch it manually.",
  reaperFound: "REAPER found",

  // Lobby
  yourName: "Your name",
  sessionsOnNetwork: "Sessions on this network",
  noSessionsYet: "No sessions yet",
  noSessionsHint: "Sessions hosted by others on your network appear here automatically.",
  setPassword: "Set a password…",
  passwordSetLabel: "Password set",
  filesAndSize: "{count} files · {size}",
  join: "Join",
  password: "Password",
  passwordOptional: "Password (optional)",
  showPassword: "Show password",
  hidePassword: "Hide password",
  hostAction: "Host",
  untitledProject: "Untitled project",
  shareProject: "Share this project with your network",
  trustTitle: "Only join people you trust",
  trustBody:
    "Joining {name} downloads and opens the host's project. Its effects and plugins run as code on your computer.",
  cancel: "Cancel",
  joinAnyway: "Join",

  // Session
  sessionFallback: "Session",
  hostingBadge: "Hosting",
  connectedBadge: "Connected",
  justYou: "Just you so far",
  peopleInSession: "{count} people in this session",
  stopHosting: "Stop hosting",
  leaveSession: "Leave session",
  youBadge: "you",
  sessionHost: "Session host",
  stopHostingTitle: "Stop hosting?",
  stopHostingBodyOne: "One other person is connected. Stopping the session disconnects them.",
  stopHostingBodyMany:
    "{count} other people are connected. Stopping the session disconnects them all.",

  // Downloading
  joiningLabel: "Joining",
  downloadingProject: "Downloading the project…",
  phaseConnect: "Connect",
  phaseDownload: "Download",
  phaseOpen: "Open in REAPER",
  connectingToHost: "Contacting the host…",
  openingInReaper: "Opening the project in REAPER…",
  filesRemaining: "{count} files left",
  oneFileRemaining: "1 file left",
  etaSeconds: "~{s}s left",
  etaMinutes: "~{m} min left",
  reaperBusyBanner: "REAPER isn't responding; a dialog there is likely waiting for an answer.",

  // Onboarding
  welcome: "Welcome to ReaMesh",
  tagline: "Real-time collaboration for REAPER",
  reaperNotFound: "REAPER not found",
  reaperNotFoundBody:
    "REAPER may not be installed, or it keeps its data somewhere else (a portable install).",
  change: "Change…",
  downloadReaper: "Download REAPER",
  checkAgain: "Check again",
  installExtTitle: "Install the REAPER extension",
  installExtBody:
    "ReaMesh adds a small extension to REAPER so this app can talk to it. It goes into",
  notReaperDirWarning:
    "This folder doesn't look like a REAPER data folder yet (no reaper.ini). Installing is harmless, but double-check the location in Settings if REAPER doesn't pick it up.",
  installing: "Installing…",
  installExtension: "Install extension",
  allSet: "You're all set",
  allSetBody:
    "The extension is installed. Start REAPER (or restart it if it's already running) and ReaMesh will connect automatically.",
  finish: "Finish",
  locateReaperFolder: "Locate your REAPER folder",

  // Settings
  back: "Back",
  settings: "Settings",
  reaperFolder: "REAPER folder",
  notFoundBadge: "Not found",
  useDefault: "Use default",
  dontHaveReaper: "Don't have REAPER? Download it",
  reaperExtension: "REAPER extension",
  notInstalled: "Not installed",
  installed: "Installed",
  updateAvailable: "Update available",
  reinstallExtension: "Reinstall extension",
  updateExtension: "Update extension",
  autoInstallExt: "Install and update automatically",
  restartNote: "REAPER loads the extension at startup, so restart it after installing or updating.",
  extInstalledToast: "Extension installed. Restart REAPER to load it.",
  language: "Language",
  languageSystem: "System",

  // Presence / shared
  guest: "Guest",
  joinedSessionSuffix: "joined the session",
  leftSessionSuffix: "left the session",

  // Overlay
  connecting: "Connecting…",
  expand: "Expand",
  collapse: "Collapse",
  revealPeerTrack: "Jump to {name}",
  overlaySettingsTitle: "Overlay",
  olRoster: "Participants",
  olItemTint: "Locked-item tint",

  // Activity-log / error events from the extension, rendered by code (see
  // lib/log.ts). `text` on the wire is the English fallback for unknown codes.
  hostEndedSession: "The host ended the session.",
  hostLost: "Lost the connection to the host.",
  wrongPassword: "Wrong password.",
  logBrowsing: "Searching for sessions on the network",
  logHosting: "Hosting “{name}”",
  logJoining: "Joining “{name}”. Downloading the project…",
  logJoinUnavailable: "Couldn’t join “{name}”. It may have just closed, so try again.",
  logJoinFailed: "Join failed: {reason}",
  logDownloadComplete: "Download complete. Opening the project…",
  logConnected: "Connected. You can edit together live now.",
  logPeerJoined: "Someone joined the session",
  logPeerLeft: "Someone left the session",
  logLeft: "Left the session",
  logProjectChanged: "Project changed, so the session ended.",
  logSaveFirst: "Save the project first, then host again.",
  logHostFailed: "Could not start hosting.",
  logBrowseFailed: "Could not start session discovery.",
  logWaitingMedia: "Waiting for media of an incoming item…",
  logMediaApplied: "Applied {count} item(s) after media arrived",
  logMediaCopied: "Copied {file} into the project folder so everyone has it",
  logMediaCopyFailed: "Couldn’t copy {file} into the project",
  logMediaTimeout: "Gave up waiting for media: {file}",
  logReconnecting: "Lost the host. Reconnecting to “{name}”…",
  reconnectFailed: "Couldn’t reconnect to “{name}”.",
  editLocked: "Your edit snapped back: someone else is editing it.",
  editLockedTrack: "Your edit to “{track}” snapped back: someone else is editing it.",
  editLockedBy: "{by} is editing this right now.",
  editLockedByTrack: "{by} is editing “{track}” right now.",
  peerTrackAdded: "Added track “{name}”",
  peerTrackAddedUnnamed: "A track was added",
  peerTrackRemoved: "Removed track “{name}”",
  peerTrackRemovedUnnamed: "A track was removed",
  peerTempoChanged: "Tempo map changed",
  peerMarkersChanged: "Markers / regions changed",
  peerTitleChanged: "Project info changed",
  peerNotesChanged: "Project notes changed",
  reconnectingBanner: "Connection to the host lost. Reconnecting…",
} as const;

export type MsgKey = keyof typeof en;

const de: Record<MsgKey, string> = {
  waitingForReaper: "Warte auf REAPER",
  startReaperToConnect: "Starte REAPER, um dich zu verbinden.",
  openReaper: "REAPER öffnen",
  couldntOpenReaper: "REAPER konnte nicht geöffnet werden. Starte es manuell.",
  reaperFound: "REAPER gefunden",

  yourName: "Dein Name",
  sessionsOnNetwork: "Sessions in diesem Netzwerk",
  noSessionsYet: "Noch keine Sessions",
  noSessionsHint: "Sessions, die andere in deinem Netzwerk hosten, erscheinen hier automatisch.",
  setPassword: "Passwort festlegen…",
  passwordSetLabel: "Passwort festgelegt",
  filesAndSize: "{count} Dateien · {size}",
  join: "Beitreten",
  password: "Passwort",
  passwordOptional: "Passwort (optional)",
  showPassword: "Passwort anzeigen",
  hidePassword: "Passwort verbergen",
  hostAction: "Hosten",
  untitledProject: "Unbenanntes Projekt",
  shareProject: "Teile dieses Projekt mit deinem Netzwerk",
  trustTitle: "Tritt nur Leuten bei, denen du vertraust",
  trustBody:
    "Beim Beitritt zu {name} wird das Projekt des Hosts heruntergeladen und geöffnet. Seine Effekte und Plugins laufen als Code auf deinem Computer.",
  cancel: "Abbrechen",
  joinAnyway: "Beitreten",

  sessionFallback: "Session",
  hostingBadge: "Hosting",
  connectedBadge: "Verbunden",
  justYou: "Bisher nur du",
  peopleInSession: "{count} Personen in dieser Session",
  stopHosting: "Hosting beenden",
  leaveSession: "Session verlassen",
  youBadge: "du",
  sessionHost: "Session-Host",
  stopHostingTitle: "Hosting beenden?",
  stopHostingBodyOne:
    "Eine weitere Person ist verbunden. Wenn du die Session beendest, wird sie getrennt.",
  stopHostingBodyMany:
    "{count} weitere Personen sind verbunden. Wenn du die Session beendest, werden alle getrennt.",

  joiningLabel: "Trete bei",
  downloadingProject: "Projekt wird heruntergeladen…",
  phaseConnect: "Verbinden",
  phaseDownload: "Download",
  phaseOpen: "In REAPER öffnen",
  connectingToHost: "Host wird kontaktiert…",
  openingInReaper: "Projekt wird in REAPER geöffnet…",
  filesRemaining: "{count} Dateien übrig",
  oneFileRemaining: "1 Datei übrig",
  etaSeconds: "~{s} s übrig",
  etaMinutes: "~{m} Min. übrig",
  reaperBusyBanner: "REAPER reagiert nicht, vermutlich wartet dort ein Dialog auf eine Antwort.",

  welcome: "Willkommen bei ReaMesh",
  tagline: "Echtzeit-Kollaboration für REAPER",
  reaperNotFound: "REAPER nicht gefunden",
  reaperNotFoundBody:
    "REAPER ist möglicherweise nicht installiert oder speichert seine Daten woanders (portable Installation).",
  change: "Ändern…",
  downloadReaper: "REAPER herunterladen",
  checkAgain: "Erneut prüfen",
  installExtTitle: "REAPER-Erweiterung installieren",
  installExtBody:
    "ReaMesh fügt REAPER eine kleine Erweiterung hinzu, damit diese App mit ihm sprechen kann. Sie wird installiert nach",
  notReaperDirWarning:
    "Dieser Ordner sieht noch nicht nach einem REAPER-Datenordner aus (keine reaper.ini). Die Installation ist unbedenklich. Prüfe aber den Ort in den Einstellungen, falls REAPER sie nicht lädt.",
  installing: "Installiere…",
  installExtension: "Erweiterung installieren",
  allSet: "Alles bereit",
  allSetBody:
    "Die Erweiterung ist installiert. Starte REAPER (oder starte es neu, falls es bereits läuft). ReaMesh verbindet sich dann automatisch.",
  finish: "Fertig",
  locateReaperFolder: "Wähle deinen REAPER-Ordner",

  back: "Zurück",
  settings: "Einstellungen",
  reaperFolder: "REAPER-Ordner",
  notFoundBadge: "Nicht gefunden",
  useDefault: "Standard verwenden",
  dontHaveReaper: "Kein REAPER? Hier herunterladen",
  reaperExtension: "REAPER-Erweiterung",
  notInstalled: "Nicht installiert",
  installed: "Installiert",
  updateAvailable: "Update verfügbar",
  reinstallExtension: "Erweiterung neu installieren",
  updateExtension: "Erweiterung aktualisieren",
  autoInstallExt: "Automatisch installieren und aktualisieren",
  restartNote:
    "REAPER lädt die Erweiterung beim Start, also starte es nach einer Installation oder einem Update neu.",
  extInstalledToast: "Erweiterung installiert. Starte REAPER neu, um sie zu laden.",
  language: "Sprache",
  languageSystem: "System",

  guest: "Gast",
  joinedSessionSuffix: "ist der Session beigetreten",
  leftSessionSuffix: "hat die Session verlassen",

  connecting: "Verbinde…",
  expand: "Ausklappen",
  collapse: "Einklappen",
  revealPeerTrack: "Zu {name} springen",
  overlaySettingsTitle: "Overlay",
  olRoster: "Teilnehmer",
  olItemTint: "Sperrtönung für Items",

  hostEndedSession: "Der Host hat die Session beendet.",
  hostLost: "Verbindung zum Host verloren.",
  wrongPassword: "Falsches Passwort.",
  logBrowsing: "Suche nach Sessions im Netzwerk",
  logHosting: "Hoste „{name}“",
  logJoining: "Trete „{name}“ bei. Projekt wird heruntergeladen…",
  logJoinUnavailable:
    "„{name}“ ließ sich nicht beitreten. Die Session wurde vielleicht gerade geschlossen, versuch es erneut.",
  logJoinFailed: "Beitritt fehlgeschlagen: {reason}",
  logDownloadComplete: "Download abgeschlossen. Projekt wird geöffnet…",
  logConnected: "Verbunden. Ihr könnt jetzt gemeinsam live bearbeiten.",
  logPeerJoined: "Jemand ist der Session beigetreten",
  logPeerLeft: "Jemand hat die Session verlassen",
  logLeft: "Session verlassen",
  logProjectChanged: "Projekt gewechselt, Session beendet.",
  logSaveFirst: "Speichere zuerst das Projekt und hoste dann erneut.",
  logHostFailed: "Hosting konnte nicht gestartet werden.",
  logBrowseFailed: "Session-Suche konnte nicht gestartet werden.",
  logWaitingMedia: "Warte auf Medien eines eingehenden Items…",
  logMediaApplied: "{count} Item(s) angewendet, nachdem die Medien angekommen sind",
  logMediaCopied: "{file} in den Projektordner kopiert, damit andere sie bekommen",
  logMediaCopyFailed: "{file} konnte nicht ins Projekt kopiert werden",
  logMediaTimeout: "Warten auf Medien aufgegeben: {file}",
  logReconnecting: "Host verloren. Verbinde erneut mit „{name}“…",
  reconnectFailed: "Erneutes Verbinden mit „{name}“ ist fehlgeschlagen.",
  editLocked: "Deine Änderung wurde zurückgesetzt. Jemand anderes bearbeitet sie gerade.",
  editLockedTrack: "Deine Änderung an „{track}“ wurde zurückgesetzt. Jemand anderes bearbeitet sie gerade.",
  editLockedBy: "{by} bearbeitet das gerade.",
  editLockedByTrack: "{by} bearbeitet gerade „{track}“.",
  peerTrackAdded: "Spur „{name}“ hinzugefügt",
  peerTrackAddedUnnamed: "Eine Spur wurde hinzugefügt",
  peerTrackRemoved: "Spur „{name}“ entfernt",
  peerTrackRemovedUnnamed: "Eine Spur wurde entfernt",
  peerTempoChanged: "Tempo geändert",
  peerMarkersChanged: "Marker / Regionen geändert",
  peerTitleChanged: "Projektinfo geändert",
  peerNotesChanged: "Projektnotizen geändert",
  reconnectingBanner: "Verbindung zum Host verloren. Verbinde erneut…",
};

function readStoredPref(): LangPref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "en" || v === "de" || v === "system") return v;
  } catch {
    // fall through
  }
  return "system";
}

let pref: LangPref = readStoredPref();
const listeners = new Set<() => void>();

function emit() {
  for (const l of listeners) l();
}

export function resolveLang(p: LangPref = pref): Lang {
  if (p !== "system") return p;
  const device = (navigator.language || "").toLowerCase();
  return device.startsWith("de") ? "de" : "en";
}

export function t(key: MsgKey, params?: Record<string, string | number>): string {
  let s: string = (resolveLang() === "de" ? de : en)[key];
  if (params) {
    for (const [k, v] of Object.entries(params)) s = s.split(`{${k}}`).join(String(v));
  }
  return s;
}

export function getLangPref(): LangPref {
  return pref;
}

/** Set the preference, mirror it to localStorage, and persist it backend-side
 *  (where the native macOS settings window reads it). */
export function setLangPref(p: LangPref) {
  applyPref(p);
  invoke("set_language", { pref: p }).catch(() => {});
}

function applyPref(p: LangPref) {
  if (p === pref) return;
  pref = p;
  try {
    localStorage.setItem(KEY, p);
  } catch {
    // localStorage unavailable; keep the in-memory value
  }
  emit();
}

/** Subscribe a component to language changes; returns the resolved language. */
export function useLang(): Lang {
  useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => pref,
  );
  return resolveLang();
}

/** One-time backend sync: adopt the persisted preference (the backend file is
 *  the source of truth: native settings may have changed it while the
 *  webview was gone) and follow native-side changes live. No-ops in
 *  plain-browser dev where the Tauri APIs reject. */
export function initLanguage() {
  invoke<string | null>("get_language")
    .then((v) => {
      if (v === "en" || v === "de" || v === "system") applyPref(v);
    })
    .catch(() => {});
  listen<string>("language-changed", (ev) => {
    const v = ev.payload;
    if (v === "en" || v === "de" || v === "system") applyPref(v);
  }).catch(() => {});
}
