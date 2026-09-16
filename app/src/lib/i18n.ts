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
  filesAndSize: "Files: {count} · {size}",
  join: "Join",
  password: "Password",
  passwordOptional: "Password (optional)",
  passwordTooShort: "Use at least {min} characters.",
  showPassword: "Show password",
  hidePassword: "Hide password",
  passwordProtected: "Password protected",
  identityColor: "Your color",
  identityColorN: "Color {n}",
  checkNoAnswer: "No answer from the host. Try again.",
  logBadChangeset: "Ignored an invalid incoming edit.",
  sessionContested: "Two different devices claim to host this session. Only join if you can verify who the real host is.",
  trustHost: "Host: {host}",
  hostFirstTime: "first time",
  hostSeenBefore: "joined {count}× before",
  hostAction: "Host",
  untitledProject: "Untitled project",
  shareProject: "Share this project on your network",
  trustTitle: "Join {name}?",
  trustBody:
    "ReaMesh downloads and opens the host's project. Its plugins can run code, so only join people you trust.",
  cancel: "Cancel",
  close: "Close",
  joinAnyway: "Join",
  haveInviteCode: "Have an invite code?",
  ticketPlaceholder: "Paste the invite code…",
  pasteInvite: "Paste",
  invalidTicket: "This doesn't look like a ReaMesh invite code.",
  ticketJoinHint:
    "Invite codes also work outside this network, for example over a VPN such as Tailscale.",

  // Join preview / trust dialog
  previewTracks: "{count} tracks",
  previewFiles: "{count} files · {size}",
  joinNoFx: "Join without effects",
  joinNoFxHint: "View-only copy, all effect chains removed. No third-party code runs.",
  pluginsAllInstalled: "All {total} plugins installed",
  pluginsMissingSummary: "{missing} of {total} plugins missing",
  pluginsUnknown: "{total} plugins · not checked",
  copyMissing: "Copy missing",
  missingCopied: "Copied",
  pluginInstalled: "Installed",
  pluginMissing: "Not installed",
  pluginUnknown: "Can't tell",

  // Pre-host warning
  hostCopyTitle: "Before you host",
  hostCopyBody:
    "Files outside the project: {count} ({size}). ReaMesh will copy them into the project folder so guests can download them.",
  hostFreeOk: "{free} free on that drive.",
  hostFreeNotEnough: "That drive only has {free} free, which isn't enough.",
  hostCloudWarning:
    "This project is in a cloud-synced folder such as Dropbox, OneDrive, or iCloud. To avoid file conflicts, pause cloud syncing while you host.",
  hostAnyway: "Host anyway",

  // Post-download plugin verification
  verifyTitle: "The plugin list was incomplete",
  verifyBodyListed:
    "The downloaded project uses plugins missing from the host's list ({count} total):",
  verifyBodyNoList: "The host didn't provide a plugin list. The downloaded project uses these plugins:",
  verifyContinue: "Open anyway",

  // Join-without-effects session badge
  nofxBadge: "Effects off",
  nofxBadgeHint: "You joined without effects. The project is view-only, and your edits aren't shared.",
  // Slow-link notice (ping round trip to the host / slowest guest is high).
  slowLinkTitle: "Slow connection to {who}",
  slowLinkHost: "the host",
  slowLinkBody:
    "Edits take a moment longer to arrive on both sides. Nothing gets lost; live editing just feels less immediate.",
  slowLinkBodyDownload: "The download will take longer than usual. It continues in the background.",
  slowLinkRtt: "Round trip: {ms} ms",

  // Session
  sessionFallback: "Session",
  hostingBadge: "Hosting",
  connectedBadge: "Connected",
  justYou: "Just you so far",
  peopleInSession: "People in this session: {count}",
  stopHosting: "Stop hosting",
  leaveSession: "Leave session",
  youBadge: "you",
  sessionHost: "Session host",
  stopHostingTitle: "Stop hosting?",
  stopHostingBodyOne: "One other person is connected. They'll be disconnected if you stop hosting.",
  stopHostingBodyMany:
    "{count} other people are connected. They'll all be disconnected if you stop hosting.",
  copyInvite: "Copy invite code",
  inviteCopied: "Invite code copied",
  inviteTicketHint:
    "Send this code to anyone you want to invite. They can paste it in their lobby to join, even over a VPN such as Tailscale. The code works while you host this session.",

  // Downloading
  downloadingProject: "Downloading the project…",
  connectingToHost: "Contacting the host…",
  openingInReaper: "Opening the project in REAPER…",
  downloadReady: "Ready",
  etaAboutSeconds: "About {s}s to go",
  etaAboutMinutes: "About {m} min to go",
  reaperBusyBanner: "REAPER isn't responding. Check whether it has a dialog waiting for you.",

  // Onboarding
  welcome: "Set up ReaMesh",
  tagline: "Real-time collaboration for REAPER",
  reaperNotFound: "REAPER not found",
  reaperNotFoundBody:
    "REAPER may not be installed, or you may be using a portable install with its data stored elsewhere.",
  change: "Change…",
  downloadReaper: "Download REAPER",
  checkAgain: "Check again",
  installExtTitle: "Install the REAPER extension",
  installExtBody:
    "ReaMesh adds a small extension that lets the app connect to REAPER. It will be installed here:",
  notReaperDirWarning:
    "This folder doesn't appear to be a REAPER data folder yet because it has no reaper.ini file. You can still install here, but check the location in Settings if REAPER doesn't find the extension.",
  installing: "Installing…",
  installExtension: "Install extension",
  allSet: "Extension installed",
  allSetBody:
    "Start REAPER, or restart it if it's already open. ReaMesh will connect automatically.",
  finish: "Finish",
  locateReaperFolder: "Locate your REAPER folder",

  // Windows Firewall (Windows only). Shown *before* the prompt appears.
  // ReaMesh is silently unusable if it gets dismissed.
  firewallTitle: "Windows will ask for network access",
  firewallBody:
    "The first time REAPER runs with ReaMesh, Windows Defender Firewall asks for permission. Select “Private networks,” then choose “Allow access.” Without it, ReaMesh can't connect to anyone on your network.",
  firewallNamesReaper:
    "The prompt says REAPER because ReaMesh runs inside it.",
  firewallAlreadyDismissed: "Closed the prompt already?",
  firewallRecover:
    "Open Windows Security, then go to Firewall & network protection → Allow an app through firewall. Enable Private for REAPER.",
  firewallGotIt: "Got it",

  // Settings
  back: "Back",
  settings: "Settings",
  minimizeWindow: "Minimize",
  maximizeWindow: "Maximize",
  restoreWindow: "Restore",
  closeWindow: "Close",
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
  restartNote: "REAPER only loads the extension when it starts. Restart REAPER after installing or updating it.",
  extInstalledToast: "Extension installed. Restart REAPER to load it.",
  extUpdatedRestart: "The REAPER extension was updated. Restart REAPER to load the new version.",
  removeExtension: "Remove extension",
  extRemovedToast: "Extension removed. Restart REAPER to unload it.",
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
  olItemTint: "Tint locked items",
  shareMarkerTitle: "Share a marker",
  markerNamePlaceholder: "Marker name…",
  share: "Share",
  followMode: "Follow someone",
  pickPersonToFollow: "Click a person to follow them",
  stopFollowing: "Stop following",
  followingName: "Following {name}",
  followingYouOne: "Following you",
  followingYouMany: "{count} following you",
  reaperNotRespondingPeer: "Their REAPER isn't responding",

  // Activity-log / error events from the extension, rendered by code (see
  // lib/log.ts). `text` on the wire is the English fallback for unknown codes.
  hostEndedSession: "The host ended the session.",
  hostLost: "Lost the connection to the host.",
  wrongPassword: "Wrong password.",
  logBrowsing: "Searching for sessions on the network",
  logHosting: "Hosting “{name}”",
  logJoining: "Joining “{name}”. The project is downloading…",
  logJoinUnavailable: "Couldn’t join “{name}”. The session may have just ended. Try again.",
  logJoinFailed: "Join failed: {reason}",
  logManifestTimeout: "The host never sent the project file list. Try joining again.",
  logDownloadStalled: "The download stopped partway through. Try joining again.",
  logHostUnreachable: "Couldn't reach the host. They may have stopped hosting.",
  logDownloadComplete: "Download complete. Opening the project…",
  logConnected: "Connected. Changes now sync live.",
  logPeerJoined: "Someone joined the session",
  logPeerLeft: "Someone left the session",
  logLeft: "Left the session",
  logProjectChanged: "Project changed, so the session ended.",
  logSaveFirst: "Save the project first, then host again.",
  logHostFailed: "Could not start hosting.",
  logBrowseFailed: "Could not start session discovery.",
  logWaitingMedia: "Waiting for media of an incoming item…",
  logMediaApplied: "Media received. Items restored: {count}",
  logMediaCopied: "Copied to the project folder for everyone: {file}",
  logMediaCopyFailed: "Couldn’t copy into the project folder: {file}",
  logMediaTimeout: "Media never arrived: {file}",
  logReconnecting: "Connection to the host lost. Reconnecting to “{name}”…",
  reconnectFailed: "Couldn’t restore the connection to “{name}”.",
  logBadTicket: "That invite code doesn’t look valid. Copy it again from the host.",
  ticketUnreachable: "Couldn’t reach that host. Make sure they're still hosting and you're on the same network or VPN.",
  editLocked: "That edit was undone because someone else is editing the same thing.",
  editLockedTrack: "Your edit to “{track}” was undone because someone else is editing that track.",
  editLockedBy: "{by} is editing this right now.",
  editLockedByTrack: "{by} is editing “{track}” right now.",
  peerTrackAdded: "Added track “{name}”",
  peerTrackAddedUnnamed: "A track was added",
  peerTrackRemoved: "Removed track “{name}”",
  peerTrackRemovedUnnamed: "A track was removed",
  peerTempoChanged: "Tempo map changed",
  peerMarkersChanged: "Markers and regions changed",
  peerTitleChanged: "Project info changed",
  peerNotesChanged: "Project notes changed",
  reconnectingBanner: "Connection to the host lost. Reconnecting…",
  logMarkerShared: "Shared marker “{name}”",
  logMarkerReceived: "{by} shared marker “{name}”",
  logMarkerReceivedUnnamed: "Marker “{name}” was shared",
  pluginAddedByTrack: "{by} added {plugin} to “{track}”",
  pluginAddedBy: "{by} added {plugin}",
  pluginAddedTrack: "{plugin} was added to “{track}”",
  pluginAdded: "{plugin} was added",
  pluginsAddedTrack: "Plugins added to “{track}”: {count}",
  pluginsAdded: "Plugins added: {count}",
  pluginNotInstalledHere: "It isn't installed here, so it'll appear offline.",
  followEnded: "You stopped following because they left the session.",
  nofxOpen: "Opening a view-only copy without effects.",
  verifyHold: "The downloaded project loads plugins the host didn't list.",

  // Download
  downloadStalled: "The download isn't moving. The host may be offline.",
  downloadProgress: "Download progress",
  fileOfTotal: "File {done} of {total}",

  // Disconnected variants
  reaperClosed: "REAPER was closed",
  reaperClosedBody: "Start it again to reconnect.",
  reaperClosedSession: "The session ended when REAPER closed.",
  portTroubleTitle: "ReaMesh is running, but every port is taken",
  portTroubleBody:
    "The extension is loaded, but all four ReaMesh ports are already in use by other REAPER instances or programs. Close one of them and ReaMesh will connect automatically. You don't need to reinstall anything.",

  // Windows Firewall: an existing block detected
  fwBlockedTitle: "Windows is blocking REAPER",
  fwBlockedBody:
    "A firewall rule is blocking incoming connections to REAPER. This often happens after the permission prompt is dismissed. Remove the rule so others on your network can reach you.",
  fwOpenSettings: "Open firewall settings",
  fwCopyFix: "Copy fix",
  fwFixCopied: "Copied. Paste it into PowerShell running as administrator.",

  // Settings: network identity
  networkIdentity: "Network identity",
  resetIdentity: "Reset network identity",
  resetIdentityHint:
    "This installation has a stable, anonymous peer ID so others can recognize you across sessions. If you reset it, ReaMesh creates a new ID the next time you join or host.",
  identityWasReset: "Identity reset. ReaMesh will create a new one for your next session.",
  noIdentityYet: "No stored identity yet.",

  // A11y labels
  peopleList: "People in this session",
  reaperInstances: "REAPER instances",
  copyInviteFailed: "Couldn't copy the code. Select and copy it manually:",
  inviteWaitsInLobby: "Invite code received. It will be waiting in the lobby once you leave this session.",

  // Crash fallback
  crashTitle: "ReaMesh hit a problem",
  crashBody:
    "The window couldn't finish loading, but your REAPER session is still running. Reload the app to continue.",
  crashReload: "Reload",
  crashCopy: "Copy details",
  crashCopied: "Copied",
} as const;

export type MsgKey = keyof typeof en;

const de: Record<MsgKey, string> = {
  waitingForReaper: "Warte auf REAPER",
  startReaperToConnect: "Starte REAPER. ReaMesh verbindet sich dann automatisch.",
  openReaper: "REAPER öffnen",
  couldntOpenReaper: "REAPER konnte nicht geöffnet werden. Starte es manuell.",
  reaperFound: "REAPER gefunden",

  yourName: "Dein Name",
  sessionsOnNetwork: "Sessions in diesem Netzwerk",
  noSessionsYet: "Noch keine Sessions",
  noSessionsHint: "Sessions, die andere in deinem Netzwerk hosten, erscheinen hier automatisch.",
  setPassword: "Passwort festlegen…",
  passwordSetLabel: "Passwort festgelegt",
  filesAndSize: "Dateien: {count} · {size}",
  join: "Beitreten",
  password: "Passwort",
  passwordOptional: "Passwort (optional)",
  passwordTooShort: "Verwende mindestens {min} Zeichen.",
  showPassword: "Passwort anzeigen",
  hidePassword: "Passwort verbergen",
  passwordProtected: "Passwortgeschützt",
  identityColor: "Deine Farbe",
  identityColorN: "Farbe {n}",
  checkNoAnswer: "Keine Antwort vom Host. Versuch es noch einmal.",
  logBadChangeset: "Ungültige Änderung empfangen und ignoriert.",
  sessionContested: "Zwei verschiedene Geräte geben sich als Host dieser Session aus. Tritt nur bei, wenn du den echten Host eindeutig erkennen kannst.",
  trustHost: "Host: {host}",
  hostFirstTime: "erstes Mal",
  hostSeenBefore: "{count}× beigetreten",
  hostAction: "Hosten",
  untitledProject: "Unbenanntes Projekt",
  shareProject: "Projekt im Netzwerk freigeben",
  trustTitle: "{name} beitreten?",
  trustBody:
    "ReaMesh lädt das Projekt des Hosts herunter und öffnet es. Seine Plugins können Code ausführen, tritt also nur Leuten bei, denen du vertraust.",
  cancel: "Abbrechen",
  close: "Schließen",
  joinAnyway: "Beitreten",
  haveInviteCode: "Hast du einen Einladungscode?",
  ticketPlaceholder: "Einladungscode einfügen…",
  pasteInvite: "Einfügen",
  invalidTicket: "Das sieht nicht nach einem ReaMesh-Einladungscode aus.",
  ticketJoinHint:
    "Einladungscodes funktionieren auch außerhalb dieses Netzwerks, zum Beispiel über ein VPN wie Tailscale.",

  previewTracks: "{count} Tracks",
  previewFiles: "{count} Dateien · {size}",
  joinNoFx: "Ohne Effekte beitreten",
  joinNoFxHint: "Schreibgeschützte Kopie ohne Effektketten. Es läuft kein Code von Drittanbietern.",
  pluginsAllInstalled: "Alle {total} Plugins installiert",
  pluginsMissingSummary: "{missing} von {total} Plugins fehlen",
  pluginsUnknown: "{total} Plugins · nicht geprüft",
  copyMissing: "Fehlende kopieren",
  missingCopied: "Kopiert",
  pluginInstalled: "Installiert",
  pluginMissing: "Nicht installiert",
  pluginUnknown: "Nicht feststellbar",

  hostCopyTitle: "Bevor du hostest",
  hostCopyBody:
    "Dateien außerhalb des Projekts: {count} ({size}). ReaMesh kopiert sie in den Projektordner, damit Gäste sie herunterladen können.",
  hostFreeOk: "{free} frei auf dem Laufwerk.",
  hostFreeNotEnough: "Auf dem Laufwerk sind nur {free} frei. Das reicht nicht aus.",
  hostCloudWarning:
    "Dieses Projekt liegt in einem synchronisierten Cloud-Ordner, etwa bei Dropbox, OneDrive oder iCloud. Pausiere den Cloud-Sync während des Hostings, um Dateikonflikte zu vermeiden.",
  hostAnyway: "Trotzdem hosten",

  verifyTitle: "Plugin-Liste unvollständig",
  verifyBodyListed:
    "Das heruntergeladene Projekt verwendet Plugins, die nicht in der Liste des Hosts standen (insgesamt {count}):",
  verifyBodyNoList: "Der Host hat keine Plugin-Liste angegeben. Das heruntergeladene Projekt verwendet diese Plugins:",
  verifyContinue: "Trotzdem öffnen",

  nofxBadge: "Effekte aus",
  nofxBadgeHint: "Du bist ohne Effekte beigetreten. Das Projekt ist schreibgeschützt und deine Änderungen werden nicht geteilt.",
  slowLinkTitle: "Langsame Verbindung zu {who}",
  slowLinkHost: "dem Host",
  slowLinkBody:
    "Änderungen brauchen in beide Richtungen etwas länger. Es geht nichts verloren, die Live-Bearbeitung fühlt sich nur weniger direkt an.",
  slowLinkBodyDownload: "Der Download dauert länger als üblich. Er läuft im Hintergrund weiter.",
  slowLinkRtt: "Antwortzeit: {ms} ms",

  sessionFallback: "Session",
  hostingBadge: "Hosting",
  connectedBadge: "Verbunden",
  justYou: "Bisher nur du",
  peopleInSession: "Personen in dieser Session: {count}",
  stopHosting: "Hosting beenden",
  leaveSession: "Session verlassen",
  youBadge: "du",
  sessionHost: "Session-Host",
  stopHostingTitle: "Hosting beenden?",
  stopHostingBodyOne:
    "Eine weitere Person nimmt teil. Wenn du das Hosting beendest, wird ihre Verbindung getrennt.",
  stopHostingBodyMany:
    "{count} weitere Personen nehmen teil. Wenn du das Hosting beendest, werden ihre Verbindungen getrennt.",
  copyInvite: "Einladungscode kopieren",
  inviteCopied: "Einladungscode kopiert",
  inviteTicketHint:
    "Schick diesen Code an alle, die du einladen möchtest. Sie können ihn in ihrer Lobby einfügen und auch über ein VPN wie Tailscale beitreten. Der Code gilt, solange du diese Session hostest.",

  downloadingProject: "Projekt wird heruntergeladen…",
  connectingToHost: "Host wird kontaktiert…",
  openingInReaper: "Projekt wird in REAPER geöffnet…",
  downloadReady: "Bereit",
  etaAboutSeconds: "Noch etwa {s} s",
  etaAboutMinutes: "Noch etwa {m} Min.",
  reaperBusyBanner: "REAPER reagiert nicht. Prüfe, ob dort ein Dialog auf deine Eingabe wartet.",

  welcome: "ReaMesh einrichten",
  tagline: "Gemeinsam in Echtzeit an REAPER-Projekten arbeiten",
  reaperNotFound: "REAPER nicht gefunden",
  reaperNotFoundBody:
    "REAPER ist möglicherweise nicht installiert. Bei einer portablen Installation können die Daten auch an einem anderen Ort liegen.",
  change: "Ändern…",
  downloadReaper: "REAPER herunterladen",
  checkAgain: "Erneut prüfen",
  installExtTitle: "REAPER-Erweiterung installieren",
  installExtBody:
    "ReaMesh installiert eine kleine Erweiterung, über die sich die App mit REAPER verbindet. Sie kommt in diesen Ordner:",
  notReaperDirWarning:
    "Dieser Ordner scheint noch kein REAPER-Datenordner zu sein, da die Datei reaper.ini fehlt. Du kannst die Erweiterung trotzdem hier installieren. Falls REAPER sie nicht findet, prüfe den Ordner in den Einstellungen.",
  installing: "Wird installiert…",
  installExtension: "Erweiterung installieren",
  allSet: "Erweiterung installiert",
  allSetBody:
    "Starte REAPER oder öffne es neu, falls es bereits läuft. ReaMesh verbindet sich dann automatisch.",
  finish: "Fertig",
  locateReaperFolder: "Wähle deinen REAPER-Ordner",

  firewallTitle: "Windows fragt nach Netzwerkzugriff",
  firewallBody:
    "Wenn REAPER zum ersten Mal mit ReaMesh läuft, fragt die Windows-Defender-Firewall nach einer Berechtigung. Aktiviere „Private Netzwerke“ und wähle dann „Zugriff zulassen“. Sonst kann sich ReaMesh mit niemandem in deinem Netzwerk verbinden.",
  firewallNamesReaper:
    "Im Dialog steht REAPER, weil ReaMesh darin ausgeführt wird.",
  firewallAlreadyDismissed: "Berechtigungsdialog schon geschlossen?",
  firewallRecover:
    "Öffne Windows-Sicherheit und gehe zu Firewall- & Netzwerkschutz → Zulassen einer App durch die Firewall. Aktiviere dort „Privat“ für REAPER.",
  firewallGotIt: "Verstanden",

  back: "Zurück",
  settings: "Einstellungen",
  minimizeWindow: "Minimieren",
  maximizeWindow: "Maximieren",
  restoreWindow: "Wiederherstellen",
  closeWindow: "Schließen",
  reaperFolder: "REAPER-Ordner",
  notFoundBadge: "Nicht gefunden",
  useDefault: "Standard verwenden",
  dontHaveReaper: "REAPER noch nicht installiert? Jetzt herunterladen",
  reaperExtension: "REAPER-Erweiterung",
  notInstalled: "Nicht installiert",
  installed: "Installiert",
  updateAvailable: "Update verfügbar",
  reinstallExtension: "Erweiterung neu installieren",
  updateExtension: "Erweiterung aktualisieren",
  autoInstallExt: "Automatisch installieren und aktualisieren",
  restartNote:
    "REAPER lädt die Erweiterung nur beim Start. Öffne REAPER nach einer Installation oder einem Update neu.",
  extInstalledToast: "Erweiterung installiert. Starte REAPER neu, um sie zu laden.",
  extUpdatedRestart: "Die REAPER-Erweiterung wurde aktualisiert. Starte REAPER neu, um die neue Version zu laden.",
  removeExtension: "Erweiterung entfernen",
  extRemovedToast: "Erweiterung entfernt. Starte REAPER neu, damit die Änderung wirksam wird.",
  language: "Sprache",
  languageSystem: "System",

  guest: "Gast",
  joinedSessionSuffix: "ist der Session beigetreten",
  leftSessionSuffix: "hat die Session verlassen",

  connecting: "Verbindung wird hergestellt…",
  expand: "Ausklappen",
  collapse: "Einklappen",
  revealPeerTrack: "Zu {name} springen",
  overlaySettingsTitle: "Overlay",
  olRoster: "Teilnehmer",
  olItemTint: "Gesperrte Items einfärben",
  shareMarkerTitle: "Marker teilen",
  markerNamePlaceholder: "Marker-Name…",
  share: "Teilen",
  followMode: "Jemandem folgen",
  pickPersonToFollow: "Klicke auf eine Person, um ihr zu folgen",
  stopFollowing: "Nicht mehr folgen",
  followingName: "Du folgst {name}",
  followingYouOne: "Folgt dir",
  followingYouMany: "{count} folgen dir",
  reaperNotRespondingPeer: "REAPER reagiert bei dieser Person nicht",

  hostEndedSession: "Der Host hat die Session beendet.",
  hostLost: "Verbindung zum Host verloren.",
  wrongPassword: "Falsches Passwort.",
  logBrowsing: "Suche im Netzwerk nach Sessions",
  logHosting: "Session „{name}“ wird gehostet",
  logJoining: "Beitritt zu „{name}“. Das Projekt wird heruntergeladen…",
  logJoinUnavailable:
    "Beitritt zu „{name}“ nicht möglich. Die Session wurde vielleicht gerade beendet. Versuch es noch einmal.",
  logJoinFailed: "Beitritt fehlgeschlagen: {reason}",
  logManifestTimeout: "Der Host hat keine Dateiliste für das Projekt gesendet. Versuch den Beitritt noch einmal.",
  logDownloadStalled: "Der Download wurde unterbrochen. Versuch den Beitritt noch einmal.",
  logHostUnreachable: "Der Host war nicht erreichbar. Vielleicht wurde die Session bereits beendet.",
  logDownloadComplete: "Download abgeschlossen. Projekt wird geöffnet…",
  logConnected: "Verbunden. Änderungen werden jetzt live synchronisiert.",
  logPeerJoined: "Jemand ist der Session beigetreten",
  logPeerLeft: "Jemand hat die Session verlassen",
  logLeft: "Session verlassen",
  logProjectChanged: "Projektwechsel erkannt. Die Session wurde beendet.",
  logSaveFirst: "Speichere zuerst das Projekt und hoste dann erneut.",
  logHostFailed: "Hosting konnte nicht gestartet werden.",
  logBrowseFailed: "Session-Suche konnte nicht gestartet werden.",
  logWaitingMedia: "Warte auf die Mediendatei eines neuen Items…",
  logMediaApplied: "Items nach dem Mediendownload übernommen: {count}",
  logMediaCopied: "Datei für alle in den Projektordner kopiert: {file}",
  logMediaCopyFailed: "Datei konnte nicht in den Projektordner kopiert werden: {file}",
  logMediaTimeout: "Mediendatei nicht erhalten: {file}",
  logReconnecting: "Verbindung zum Host verloren. ReaMesh verbindet sich erneut mit „{name}“…",
  reconnectFailed: "ReaMesh konnte die Verbindung zu „{name}“ nicht wiederherstellen.",
  logBadTicket: "Dieser Einladungscode scheint ungültig zu sein. Kopiere ihn beim Host erneut.",
  ticketUnreachable: "Host nicht erreichbar. Prüfe, ob die Session noch läuft und ihr im selben Netzwerk oder VPN seid.",
  editLocked: "Diese Änderung wurde rückgängig gemacht, weil jemand anderes gerade dasselbe bearbeitet.",
  editLockedTrack: "Deine Änderung an „{track}“ wurde rückgängig gemacht, weil jemand anderes die Spur gerade bearbeitet.",
  editLockedBy: "{by} bearbeitet das gerade.",
  editLockedByTrack: "{by} bearbeitet gerade „{track}“.",
  peerTrackAdded: "Spur „{name}“ hinzugefügt",
  peerTrackAddedUnnamed: "Eine Spur wurde hinzugefügt",
  peerTrackRemoved: "Spur „{name}“ entfernt",
  peerTrackRemovedUnnamed: "Eine Spur wurde entfernt",
  peerTempoChanged: "Tempo geändert",
  peerMarkersChanged: "Marker und Regionen geändert",
  peerTitleChanged: "Projektinfo geändert",
  peerNotesChanged: "Projektnotizen geändert",
  reconnectingBanner: "Verbindung zum Host verloren. Sie wird wiederhergestellt…",
  logMarkerShared: "Marker „{name}“ geteilt",
  logMarkerReceived: "{by} hat den Marker „{name}“ geteilt",
  logMarkerReceivedUnnamed: "Marker „{name}“ wurde geteilt",
  pluginAddedByTrack: "{by} hat {plugin} zur Spur „{track}“ hinzugefügt",
  pluginAddedBy: "{by} hat {plugin} hinzugefügt",
  pluginAddedTrack: "{plugin} wurde zur Spur „{track}“ hinzugefügt",
  pluginAdded: "{plugin} wurde hinzugefügt",
  pluginsAddedTrack: "Zur Spur „{track}“ hinzugefügte Plugins: {count}",
  pluginsAdded: "Hinzugefügte Plugins: {count}",
  pluginNotInstalledHere: "Dieses Plugin ist hier nicht installiert und erscheint deshalb offline.",
  followEnded: "Du folgst der Person nicht mehr, weil sie die Session verlassen hat.",
  nofxOpen: "Schreibgeschützte Kopie wird ohne Effekte geöffnet.",
  verifyHold: "Das heruntergeladene Projekt enthält Plugins, die der Host nicht angegeben hat.",

  downloadStalled: "Der Download kommt nicht voran. Der Host ist möglicherweise offline.",
  downloadProgress: "Download-Fortschritt",
  fileOfTotal: "Datei {done} von {total}",

  reaperClosed: "REAPER wurde beendet",
  reaperClosedBody: "Starte es erneut, um dich wieder zu verbinden.",
  reaperClosedSession: "Die Session endete, als REAPER beendet wurde.",
  portTroubleTitle: "ReaMesh läuft, aber kein Port ist frei",
  portTroubleBody:
    "Die Erweiterung ist geladen, aber alle vier ReaMesh-Ports werden bereits von anderen REAPER-Instanzen oder Programmen verwendet. Schließe eines davon. ReaMesh verbindet sich danach automatisch, ohne dass du etwas neu installieren musst.",

  fwBlockedTitle: "Windows blockiert REAPER",
  fwBlockedBody:
    "Eine Firewall-Regel blockiert eingehende Verbindungen zu REAPER. Das passiert oft, wenn der Berechtigungsdialog geschlossen wurde. Entferne die Regel, damit dich andere im Netzwerk erreichen können.",
  fwOpenSettings: "Firewall-Einstellungen öffnen",
  fwCopyFix: "Lösung kopieren",
  fwFixCopied: "Kopiert. Öffne PowerShell als Administrator und füge den Befehl ein.",

  networkIdentity: "Netzwerkidentität",
  resetIdentity: "Netzwerkidentität zurücksetzen",
  resetIdentityHint:
    "Diese Installation hat eine feste, anonyme Peer-ID, an der dich andere über mehrere Sessions hinweg erkennen. Nach dem Zurücksetzen erstellt ReaMesh beim nächsten Beitritt oder Hosting eine neue ID.",
  identityWasReset: "Identität zurückgesetzt. Für die nächste Session erstellt ReaMesh eine neue.",
  noIdentityYet: "Noch keine gespeicherte Identität.",

  peopleList: "Personen in dieser Session",
  reaperInstances: "REAPER-Instanzen",
  copyInviteFailed: "Der Code konnte nicht kopiert werden. Markiere und kopiere ihn manuell:",
  inviteWaitsInLobby: "Einladungscode erhalten. Er wartet in der Lobby, sobald du diese Session verlässt.",

  crashTitle: "Bei ReaMesh ist ein Fehler aufgetreten",
  crashBody:
    "Das Fenster konnte nicht vollständig geladen werden. Deine Session in REAPER läuft weiter. Lade die App neu, um fortzufahren.",
  crashReload: "Neu laden",
  crashCopy: "Details kopieren",
  crashCopied: "Kopiert",
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
    // localStorage is unavailable, so keep the in-memory value.
  }
  syncDocumentLang();
  emit();
}

/** Set the document language so screen readers use the UI's pronunciation rules. */
function syncDocumentLang() {
  if (typeof document !== "undefined") document.documentElement.lang = resolveLang();
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
 *  the source of truth because native settings may have changed it while the
 *  webview was gone) and follow native-side changes live. No-ops in
 *  plain-browser dev where the Tauri APIs reject. */
export function initLanguage() {
  syncDocumentLang();
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
