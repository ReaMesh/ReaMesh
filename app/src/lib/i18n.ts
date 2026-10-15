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
  openReaper: "Open REAPER",
  couldntOpenReaper: "Couldn't open REAPER. Launch it manually.",
  reaperFound: "REAPER found",

  // Lobby
  yourName: "Your name",
  nameFirst: "Enter your name first, so the others know who you are.",
  sessionsOnNetwork: "Sessions on this network",
  noSessionsYet: "No sessions yet",
  vpnHint: "A VPN is on, which can hide sessions on this network. An invite code still works.",
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
  sessionContested: "Two devices claim to be the host",
  hostAction: "Host",
  untitledProject: "Untitled project",
  trustTitle: "Join {name}?",
  trustBody: "Plugins in the project can run code.",
  cancel: "Cancel",
  close: "Close",
  joinAnyway: "Join",
  haveInviteCode: "Got an invite code?",
  ticketPlaceholder: "Paste the invite code…",
  pasteInvite: "Paste",
  invalidTicket: "This doesn't look like a ReaMesh invite code.",
  ticketJoinHint:
    "Invite codes also work outside this network, for example over a VPN such as Tailscale.",

  // Join preview / trust dialog
  joinNoFxHint: "View-only copy, all effect chains removed. No third-party code runs.",
  pluginsAllInstalled: "All {total} plugins installed",
  pluginsMissingSummary: "{missing} of {total} plugins missing",
  pluginsUnknown: "{total} plugins, not checked",
  copyMissing: "Copy missing",
  missingCopied: "Copied",
  pluginInstalled: "Installed",
  pluginMissingShort: "missing",
  pluginUnknown: "Can't tell",

  // Pre-host warning
  hostCopyTitle: "Before you host",
  hostCloudWarning: "The project is in a cloud folder. Pause syncing while you host.",
  hostAnyway: "Host anyway",
  hostCopyAndHost: "Copy and host",

  // Post-download plugin verification
  verifyTitle: "The plugin list was incomplete",
  verifyBodyListed: "These plugins weren't on the host's list:",
  verifyBodyNoList: "The host sent no plugin list. The project loads these plugins:",
  verifyContinue: "Open anyway",

  // Joined without effects: the line under the session name.
  nofxSub: "Without effects (view only)",
  // Ping round trip to a peer is high.
  slowLink: "Slow connection",
  slowLinkToHost: "Slow connection to the host",
  slowLinkRtt: "Round trip: {ms} ms",

  // Session
  sessionFallback: "Session",
  stopHosting: "Stop hosting",
  leaveSession: "Leave session",
  youBadge: "you",
  sessionHost: "Session host",
  stopHostingTitle: "Stop hosting?",
  stopHostingBodyOne: "One person will be disconnected.",
  stopHostingBodyMany: "{count} people will be disconnected.",
  inviteTicketHint:
    "Send this code to anyone you want to invite. They can paste it in their lobby to join, even over a VPN such as Tailscale. The code works while you host this session.",

  // Moderation (host only): kick = they may rejoin at once; block = their
  // device is kept out for 30 minutes, revocable from the blocked list.
  banTitle: "Block {name}?",
  banBody: "{name} will be removed and can't rejoin for 30 minutes.",
  banConfirm: "Block",
  blockedSection: "Blocked",
  blockExpiresIn: "{minutes} min left",
  blockExpiresSoon: "less than a minute left",
  unblock: "Unblock",
  peerKicked: "You removed {name} from the session.",
  peerKickedUnnamed: "You removed a guest from the session.",
  peerBanned: "You blocked {name} for 30 minutes.",
  peerBannedUnnamed: "You blocked a guest for 30 minutes.",
  peerUnbanned: "You unblocked {name}.",
  peerUnbannedUnnamed: "You unblocked a guest.",
  kickedByHost: "The host removed you from the session. You can rejoin.",
  bannedByHost: "The host blocked you from this session for {minutes} minutes.",
  bannedFromSession: "The host has blocked you from this session.",

  // Downloading
  downloadingProject: "Downloading the project…",
  connectingToHost: "Contacting the host…",
  openingInReaper: "Opening the project in REAPER…",
  downloadReady: "Ready",
  etaAboutSeconds: "About {s}s to go",
  etaAboutMinutes: "About {m} min to go",
  reaperBusyTitle: "REAPER isn't responding",
  reaperBusyHint: "A dialog is probably open there.",
  reaperBusyShow: "Show REAPER",

  // Onboarding
  welcome: "Set up ReaMesh",
  tagline: "Real-time collaboration for REAPER",
  reaperNotFound: "REAPER not found",
  change: "Change…",
  downloadReaper: "Download REAPER",
  checkAgain: "Check again",
  installExtTitle: "Install the REAPER extension",
  notReaperDirWarning: "This folder has no reaper.ini. Is it REAPER's data folder?",
  installing: "Installing…",
  installExtension: "Install extension",
  allSet: "Extension installed",
  finish: "Finish",
  locateReaperFolder: "Locate your REAPER folder",

  // Windows Firewall (Windows only). Shown *before* the prompt appears.
  // ReaMesh is silently unusable if it gets dismissed.
  firewallBody:
    "On first start, Windows asks for network access. Choose “Private networks” and “Allow access”.",
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
  notFoundBadge: "Not found",
  useDefault: "Use default",
  dontHaveReaper: "Don't have REAPER? Download it",
  reaperExtension: "REAPER extension",
  notInstalled: "Not installed",
  installed: "Installed",
  updateAvailable: "Update available",
  extInstalledToast: "Extension installed. Restart REAPER to load it.",
  extUpdatedRestart: "Extension updated. Restart REAPER to load it.",
  extRemovedToast: "Extension removed. Restart REAPER to unload it.",
  language: "Language",
  languageSystem: "System",

  // Presence / shared
  guest: "Guest",
  joinedSessionSuffix: "joined",
  leftSessionSuffix: "left",

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
  followPerson: "Follow {name}",
  reaperNotRespondingPeer: "Their REAPER isn't responding",
  reaperNotResponding: "REAPER not responding",

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
  logProjectChanged: "You switched projects, so the session ended.",
  logSaveFirst: "Save the project first, then host again.",
  logHostFailed: "Could not start hosting.",
  logBrowseFailed: "Could not start session discovery.",
  logWaitingMedia: "Waiting for an item's media…",
  logMediaApplied: "Media arrived for {count} items.",
  logMediaAppliedOne: "Media arrived for 1 item.",
  logTintsHealed: "{count} items saved in grey got their color back.",
  logTintsHealedOne: "1 item saved in grey got its color back.",
  logMediaCopied: "Copied {file} into the project folder so everyone gets it.",
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
  reconnectingBanner: "Reconnecting…",
  reconnectingTo: "Reconnecting to “{name}”…",
  logMarkerShared: "Shared marker “{name}”",
  logMarkerReceived: "{by} shared marker “{name}”",
  logMarkerReceivedUnnamed: "Marker “{name}” was shared",
  pluginAddedByTrack: "{by} added {plugin} to “{track}”",
  pluginAddedBy: "{by} added {plugin}",
  pluginAddedTrack: "{plugin} was added to “{track}”",
  pluginAdded: "{plugin} was added",
  pluginsAddedTrack: "{count} plugins added to “{track}”",
  pluginsAdded: "{count} plugins added",
  pluginNotInstalledHere: "Not installed here, so it shows offline.",
  followEnded: "Follow mode ended because they left the session.",
  nofxOpen: "Opening a view-only copy without effects.",
  verifyHold: "The downloaded project loads plugins the host didn't list.",

  // REAPER version: minimum gate + host mismatch
  reaperOutdatedTitle: "REAPER needs an update",
  reaperOutdatedBody: "ReaMesh needs REAPER {min} or newer. You have {version}.",
  reaperOutdatedBodyUnknown: "ReaMesh needs REAPER {min} or newer.",
  getReaperUpdate: "Get the REAPER update",
  reaperYou: "{host} (you: {local})",
  reaperMismatchHostNewer:
    "The host runs REAPER {host}, you run {local}. Parts of the project may not load correctly here. Updating REAPER is recommended.",
  reaperMismatchHostOlder:
    "The host runs REAPER {host}, you run {local}. Some of your edits may not arrive cleanly on their side.",
  logReaperMismatch: "The host runs REAPER {host}, you run {local}. Some edits may not sync cleanly.",

  // Progressive join: the project opens while media is still downloading.
  logProgressiveOpen: "Project opened. Items waiting for their media: {count}",
  logLoadingReadonly: "The project is still loading. Editing unlocks once everything is here.",
  logMediaDownloaded: "All media files downloaded.",
  logMediaReady: "Everything is here. You can edit now.",
  logPlaceholdersUnresolved: "{count} items never got their media and stay empty here.",
  logPlaceholdersUnresolvedOne: "1 item never got its media and stays empty here.",
  logPlaceholderBlocked: "An item still waiting for its media wasn't synced.",
  loadingMedia: "Loading media",

  // Download
  downloadStalled: "The download isn't moving. The host may be offline.",
  downloadProgress: "Download progress",
  bytesOf: "{done} of {total}",

  // Disconnected variants
  reaperClosed: "REAPER was closed",
  reaperClosedSession: "The session ended when REAPER closed.",
  portTrouble: "Every ReaMesh port is taken. Close another REAPER instance and ReaMesh connects on its own.",

  // Windows Firewall: an existing block detected
  fwBlocked: "A firewall rule blocks REAPER. Others on your network can't reach you.",
  fwOpenSettings: "Open firewall settings",
  fwCopyFix: "Copy fix",
  fwFixCopied: "Copied. Paste it into PowerShell running as administrator.",

  // Settings: network identity
  networkIdentity: "Network identity",
  resetIdentityHint:
    "This installation has a stable, anonymous peer ID so others can recognize you across sessions. If you reset it, ReaMesh creates a new ID the next time you join or host.",
  identityWasReset: "Identity reset. ReaMesh will create a new one for your next session.",
  noIdentityYet: "No stored identity yet.",

  // A11y labels
  peopleList: "People in this session",
  reaperInstances: "REAPER instances",
  inviteWaitsInLobby: "Invite code received. It will be waiting in the lobby once you leave this session.",

  // Crash fallback
  crashTitle: "ReaMesh hit a problem",
  crashBody:
    "The window couldn't finish loading, but your REAPER session is still running. Reload the app to continue.",
  crashReload: "Reload",
  crashCopy: "Copy details",
  crashCopied: "Copied",
  trackN: "Track {n}",
  editProfile: "Edit profile",
  profile: "Profile",
  trustHostLabel: "Host",
  trustSizeLabel: "Size",
  hostSeenTimes: "you joined {count}×",
  hostSeenNever: "first time",
  joinModeLabel: "Effects",
  joinWithFx: "With effects",
  joinWithoutFx: "Without effects",
  passwordFor: "Password for {name}",
  hostCopyLabel: "Copy into the project folder",
  hostFreeLabel: "Free",
  hostFreeShort: "not enough",
  peerEditing: "editing {what}",
  followsYou: "follows you",
  followsName: "follows {name}",
  followingShort: "Following",
  actionsFor: "Actions for {name}",
  followAction: "Follow",
  jumpToTrack: "Jump to track",
  kickAction: "Remove",
  banAction: "Block…",
  copy: "Copy",
  copied: "Copied",
  inviteLink: "Invite link",
  inviteCode: "Invite code",
  hide: "Hide",
  show: "Show",
  people: "People",
  invite: "Invite",
  quoted: "“{text}”",
  editingTempo: "the tempo map",
  editingMarkers: "markers",
  editingProject: "project settings",
  olRememberPos: "Remember HUD position",
  olShow: "Show overlay",
  privacy: "Privacy",
  help: "Help",
  documentation: "Documentation",
  copyDiagnostics: "Copy diagnostics",
  diagnosticsCopied: "Copied",
  installShort: "Install",
  updateShort: "Update",
  removeShort: "Remove",
  resetShort: "Reset",
  restartReaperStep: "Restart REAPER",
  endFollow: "End",
  markerTool: "Marker",
  // Versions and in-app updates
  hostNewerVersion: "The host runs a newer ReaMesh. Update to join.",
  hostOlderVersion: "The host runs an older ReaMesh version.",
  hostOtherVersion: "This host runs a different ReaMesh version.",
  logVersionMismatch: "The host runs a different ReaMesh version.",
  extOldTitle: "Restart REAPER",
  extOldBody: "ReaMesh was updated. Restart REAPER so it loads the matching extension.",
  appOldTitle: "Update ReaMesh",
  appOldBody: "REAPER runs a newer ReaMesh extension than this app.",
  updates: "Updates",
  checkUpdates: "Check",
  upToDate: "Up to date",
  updateChecking: "Checking…",
  updateDownloading: "Downloading {version}…",
  updateReady: "{version} is ready",
  updateReadyAfterSession: "{version} is ready. You can restart after the session.",
  updateCheckFailed: "Couldn't check for updates",
  restartNow: "Restart",
  later: "Later",
  autoCheckUpdates: "Check for updates automatically",
  updateReadyTitle: "ReaMesh {version} is ready",
  updateReloadHint: "The window reloads briefly.",
  updateAfterSession: "You can restart after the session.",
  updateDownloadingTip: "Downloading update {version}, {pct}%",
  updateChip: "Update",
  extRestartToLoad: "Restart REAPER to load {version}",
  versionLabel: "Version {version}",
  credits: "Acknowledgements",
  builtWith: "Built with",
  allPackages: "All packages",
  aboutTitle: "About ReaMesh",
  aboutVersion: "Version",
  aboutBuild: "Build",
  aboutProtocol: "Protocol",
  releaseBuild: "Release build",
  devBuild: "Dev build",
  sourceCode: "Source code",
  aboutTagline: "Realtime, peer-to-peer collaboration for REAPER",
  cockosNotice: "REAPER is a trademark of Cockos Incorporated. ReaMesh is not affiliated with or endorsed by Cockos.",
} as const;

export type MsgKey = keyof typeof en;

const de: Record<MsgKey, string> = {
  waitingForReaper: "Warte auf REAPER",
  openReaper: "REAPER öffnen",
  couldntOpenReaper: "REAPER konnte nicht geöffnet werden. Starte es manuell.",
  reaperFound: "REAPER gefunden",

  yourName: "Dein Name",
  nameFirst: "Gib zuerst deinen Namen ein, damit die anderen wissen, wer du bist.",
  sessionsOnNetwork: "Sessions in diesem Netzwerk",
  noSessionsYet: "Noch keine Sessions",
  vpnHint: "Ein VPN ist aktiv. Dadurch sind Sessions im Netzwerk eventuell nicht sichtbar. Ein Einladungscode funktioniert trotzdem.",
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
  sessionContested: "Zwei Geräte geben sich als Host aus",
  hostAction: "Hosten",
  untitledProject: "Unbenanntes Projekt",
  trustTitle: "{name} beitreten?",
  trustBody: "Plugins im Projekt können Code ausführen.",
  cancel: "Abbrechen",
  close: "Schließen",
  joinAnyway: "Beitreten",
  haveInviteCode: "Hast du einen Einladungscode?",
  ticketPlaceholder: "Einladungscode einfügen…",
  pasteInvite: "Einfügen",
  invalidTicket: "Das sieht nicht nach einem ReaMesh-Einladungscode aus.",
  ticketJoinHint:
    "Einladungscodes funktionieren auch außerhalb dieses Netzwerks, zum Beispiel über ein VPN wie Tailscale.",

  joinNoFxHint: "Schreibgeschützte Kopie ohne Effektketten. Es läuft kein Code von Drittanbietern.",
  pluginsAllInstalled: "Alle {total} Plugins installiert",
  pluginsMissingSummary: "{missing} von {total} Plugins fehlen",
  pluginsUnknown: "{total} Plugins, nicht geprüft",
  copyMissing: "Fehlende kopieren",
  missingCopied: "Kopiert",
  pluginInstalled: "Installiert",
  pluginMissingShort: "fehlt",
  pluginUnknown: "Nicht feststellbar",

  hostCopyTitle: "Bevor du hostest",
  hostCloudWarning: "Das Projekt liegt in einem Cloud-Ordner. Pausiere den Sync, solange du hostest.",
  hostAnyway: "Trotzdem hosten",
  hostCopyAndHost: "Kopieren und hosten",

  verifyTitle: "Plugin-Liste unvollständig",
  verifyBodyListed: "Diese Plugins standen nicht in der Liste des Hosts:",
  verifyBodyNoList: "Der Host hat keine Plugin-Liste geschickt. Das Projekt lädt diese Plugins:",
  verifyContinue: "Trotzdem öffnen",

  nofxSub: "Ohne Effekte (nur ansehen)",
  slowLink: "Langsame Verbindung",
  slowLinkToHost: "Langsame Verbindung zum Host",
  slowLinkRtt: "Antwortzeit: {ms} ms",

  sessionFallback: "Session",
  stopHosting: "Hosting beenden",
  leaveSession: "Session verlassen",
  youBadge: "du",
  sessionHost: "Session-Host",
  stopHostingTitle: "Hosting beenden?",
  stopHostingBodyOne: "Eine Person wird getrennt.",
  stopHostingBodyMany: "{count} Personen werden getrennt.",
  inviteTicketHint:
    "Schick diesen Code an alle, die du einladen möchtest. Sie können ihn in ihrer Lobby einfügen und auch über ein VPN wie Tailscale beitreten. Der Code gilt, solange du diese Session hostest.",

  banTitle: "{name} blockieren?",
  banBody: "{name} wird entfernt und kann 30 Minuten nicht beitreten.",
  banConfirm: "Blockieren",
  blockedSection: "Blockiert",
  blockExpiresIn: "noch {minutes} Min.",
  blockExpiresSoon: "noch unter einer Minute",
  unblock: "Freigeben",
  peerKicked: "Du hast {name} aus der Session entfernt.",
  peerKickedUnnamed: "Du hast einen Gast aus der Session entfernt.",
  peerBanned: "Du hast {name} für 30 Minuten blockiert.",
  peerBannedUnnamed: "Du hast einen Gast für 30 Minuten blockiert.",
  peerUnbanned: "Du hast {name} freigegeben.",
  peerUnbannedUnnamed: "Du hast einen Gast freigegeben.",
  kickedByHost: "Der Host hat dich aus der Session entfernt. Du kannst wieder beitreten.",
  bannedByHost: "Der Host hat dich für {minutes} Minuten in dieser Session blockiert.",
  bannedFromSession: "Der Host hat dich in dieser Session blockiert.",

  downloadingProject: "Projekt wird heruntergeladen…",
  connectingToHost: "Host wird kontaktiert…",
  openingInReaper: "Projekt wird in REAPER geöffnet…",
  downloadReady: "Bereit",
  etaAboutSeconds: "Noch etwa {s} s",
  etaAboutMinutes: "Noch etwa {m} Min.",
  reaperBusyTitle: "REAPER reagiert nicht",
  reaperBusyHint: "Vermutlich ist dort ein Dialog offen.",
  reaperBusyShow: "Zu REAPER wechseln",

  welcome: "ReaMesh einrichten",
  tagline: "Gemeinsam in Echtzeit an REAPER-Projekten arbeiten",
  reaperNotFound: "REAPER nicht gefunden",
  change: "Ändern…",
  downloadReaper: "REAPER herunterladen",
  checkAgain: "Erneut prüfen",
  installExtTitle: "REAPER-Erweiterung installieren",
  notReaperDirWarning: "Dieser Ordner hat keine reaper.ini. Ist es der REAPER-Datenordner?",
  installing: "Wird installiert…",
  installExtension: "Erweiterung installieren",
  allSet: "Erweiterung installiert",
  finish: "Fertig",
  locateReaperFolder: "Wähle deinen REAPER-Ordner",

  firewallBody:
    "Beim ersten Start fragt Windows nach Netzwerkzugriff. Wähle „Private Netzwerke“ und „Zugriff zulassen“.",
  firewallNamesReaper:
    "Im Dialog steht REAPER, weil ReaMesh innerhalb von REAPER läuft.",
  firewallAlreadyDismissed: "Dialog schon geschlossen?",
  firewallRecover:
    "Öffne Windows-Sicherheit und gehe zu Firewall- & Netzwerkschutz → Zulassen einer App durch die Firewall. Aktiviere dort „Privat“ für REAPER.",
  firewallGotIt: "Verstanden",

  back: "Zurück",
  settings: "Einstellungen",
  minimizeWindow: "Minimieren",
  maximizeWindow: "Maximieren",
  restoreWindow: "Wiederherstellen",
  closeWindow: "Schließen",
  notFoundBadge: "Nicht gefunden",
  useDefault: "Standard verwenden",
  dontHaveReaper: "REAPER noch nicht installiert? Jetzt herunterladen",
  reaperExtension: "REAPER-Erweiterung",
  notInstalled: "Nicht installiert",
  installed: "Installiert",
  updateAvailable: "Update verfügbar",
  extInstalledToast: "Erweiterung installiert. Starte REAPER neu, um sie zu laden.",
  extUpdatedRestart: "Erweiterung aktualisiert. Starte REAPER neu, um sie zu laden.",
  extRemovedToast: "Erweiterung entfernt. Starte REAPER neu, damit die Änderung wirksam wird.",
  language: "Sprache",
  languageSystem: "System",

  guest: "Gast",
  joinedSessionSuffix: "ist beigetreten",
  leftSessionSuffix: "ist gegangen",

  connecting: "Verbindung wird hergestellt…",
  expand: "Ausklappen",
  collapse: "Einklappen",
  revealPeerTrack: "Zu {name} springen",
  overlaySettingsTitle: "Overlay",
  olRoster: "Teilnehmende",
  olItemTint: "Gesperrte Items einfärben",
  shareMarkerTitle: "Marker teilen",
  markerNamePlaceholder: "Marker-Name…",
  share: "Teilen",
  followMode: "Jemandem folgen",
  pickPersonToFollow: "Klicke auf eine Person, um ihr zu folgen",
  stopFollowing: "Nicht mehr folgen",
  followingName: "Du folgst {name}",
  followPerson: "{name} folgen",
  reaperNotRespondingPeer: "REAPER reagiert bei dieser Person nicht",
  reaperNotResponding: "REAPER reagiert nicht",

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
  logMediaApplied: "Medien für {count} Items angekommen.",
  logMediaAppliedOne: "Medien für 1 Item angekommen.",
  logTintsHealed: "{count} grau gespeicherte Items haben wieder ihre Farbe.",
  logTintsHealedOne: "1 grau gespeichertes Item hat wieder seine Farbe.",
  logMediaCopied: "Datei für alle in den Projektordner kopiert: {file}",
  logMediaCopyFailed: "Datei konnte nicht in den Projektordner kopiert werden: {file}",
  logMediaTimeout: "Mediendatei nicht erhalten: {file}",
  logReconnecting: "Verbindung zum Host verloren. ReaMesh verbindet sich erneut mit „{name}“…",
  reconnectFailed: "ReaMesh konnte die Verbindung zu „{name}“ nicht wiederherstellen.",
  logBadTicket: "Dieser Einladungscode scheint ungültig zu sein. Lass ihn dir vom Host noch einmal schicken.",
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
  reconnectingBanner: "Verbinde erneut…",
  reconnectingTo: "Verbinde erneut mit „{name}“…",
  logMarkerShared: "Marker „{name}“ geteilt",
  logMarkerReceived: "{by} hat den Marker „{name}“ geteilt",
  logMarkerReceivedUnnamed: "Marker „{name}“ wurde geteilt",
  pluginAddedByTrack: "{by} hat {plugin} zur Spur „{track}“ hinzugefügt",
  pluginAddedBy: "{by} hat {plugin} hinzugefügt",
  pluginAddedTrack: "{plugin} wurde zur Spur „{track}“ hinzugefügt",
  pluginAdded: "{plugin} wurde hinzugefügt",
  pluginsAddedTrack: "{count} Plugins zu „{track}“ hinzugefügt",
  pluginsAdded: "{count} Plugins hinzugefügt",
  pluginNotInstalledHere: "Hier nicht installiert, erscheint deshalb offline.",
  followEnded: "Du folgst der Person nicht mehr, weil sie die Session verlassen hat.",
  nofxOpen: "Schreibgeschützte Kopie wird ohne Effekte geöffnet.",
  verifyHold: "Das heruntergeladene Projekt enthält Plugins, die der Host nicht angegeben hat.",

  // REAPER version: minimum gate + host mismatch
  reaperOutdatedTitle: "REAPER braucht ein Update",
  reaperOutdatedBody: "ReaMesh braucht REAPER {min} oder neuer. Installiert ist {version}.",
  reaperOutdatedBodyUnknown: "ReaMesh braucht REAPER {min} oder neuer.",
  getReaperUpdate: "REAPER-Update holen",
  reaperYou: "{host} (du: {local})",
  reaperMismatchHostNewer:
    "Der Host nutzt REAPER {host}, du nutzt {local}. Teile des Projekts werden hier eventuell nicht korrekt geladen. Ein REAPER-Update wird empfohlen.",
  reaperMismatchHostOlder:
    "Der Host nutzt REAPER {host}, du nutzt {local}. Manche deiner Änderungen kommen dort eventuell nicht sauber an.",
  logReaperMismatch:
    "Der Host nutzt REAPER {host}, du nutzt {local}. Manche Änderungen werden eventuell nicht sauber synchronisiert.",

  logProgressiveOpen: "Projekt geöffnet. Items, die noch auf ihre Medien warten: {count}",
  logLoadingReadonly: "Das Projekt lädt noch. Bearbeiten geht, sobald alles da ist.",
  logMediaDownloaded: "Alle Mediendateien heruntergeladen.",
  logMediaReady: "Alles da. Du kannst jetzt bearbeiten.",
  logPlaceholdersUnresolved: "{count} Items haben ihre Medien nie erhalten und bleiben hier leer.",
  logPlaceholdersUnresolvedOne: "1 Item hat seine Medien nie erhalten und bleibt hier leer.",
  logPlaceholderBlocked: "Ein Item, das noch auf seine Medien wartet, wurde nicht synchronisiert.",
  loadingMedia: "Medien werden geladen",

  downloadStalled: "Der Download kommt nicht voran. Der Host ist möglicherweise offline.",
  downloadProgress: "Download-Fortschritt",
  bytesOf: "{done} von {total}",

  reaperClosed: "REAPER wurde beendet",
  reaperClosedSession: "Die Session endete, als REAPER beendet wurde.",
  portTrouble:
    "Alle ReaMesh-Ports sind belegt. Schließe eine andere REAPER-Instanz, dann verbindet sich ReaMesh von selbst.",

  fwBlocked: "Eine Firewall-Regel blockiert REAPER. Andere im Netzwerk erreichen dich nicht.",
  fwOpenSettings: "Firewall-Einstellungen öffnen",
  fwCopyFix: "Lösung kopieren",
  fwFixCopied: "Kopiert. Öffne PowerShell als Administrator und füge den Befehl ein.",

  networkIdentity: "Netzwerkidentität",
  resetIdentityHint:
    "Diese Installation hat eine feste, anonyme Peer-ID, an der dich andere über mehrere Sessions hinweg erkennen. Nach dem Zurücksetzen erstellt ReaMesh beim nächsten Beitritt oder Hosting eine neue ID.",
  identityWasReset: "Identität zurückgesetzt. Für die nächste Session erstellt ReaMesh eine neue.",
  noIdentityYet: "Noch keine gespeicherte Identität.",

  peopleList: "Personen in dieser Session",
  reaperInstances: "REAPER-Instanzen",
  inviteWaitsInLobby: "Einladungscode erhalten. Er wartet in der Lobby, sobald du diese Session verlässt.",

  crashTitle: "Bei ReaMesh ist ein Fehler aufgetreten",
  crashBody:
    "Das Fenster konnte nicht vollständig geladen werden. Deine Session in REAPER läuft weiter. Lade die App neu, um fortzufahren.",
  crashReload: "Neu laden",
  crashCopy: "Details kopieren",
  crashCopied: "Kopiert",
  trackN: "Spur {n}",
  editProfile: "Profil bearbeiten",
  profile: "Profil",
  trustHostLabel: "Host",
  trustSizeLabel: "Größe",
  hostSeenTimes: "du warst {count}× dabei",
  hostSeenNever: "erstes Mal",
  joinModeLabel: "Effekte",
  joinWithFx: "Mit Effekten",
  joinWithoutFx: "Ohne Effekte",
  passwordFor: "Passwort für {name}",
  hostCopyLabel: "In den Projektordner kopieren",
  hostFreeLabel: "Frei",
  hostFreeShort: "reicht nicht",
  peerEditing: "bearbeitet {what}",
  followsYou: "folgt dir",
  followsName: "folgt {name}",
  followingShort: "Du folgst",
  actionsFor: "Aktionen für {name}",
  followAction: "Folgen",
  jumpToTrack: "Zur Spur springen",
  kickAction: "Entfernen",
  banAction: "Blockieren…",
  copy: "Kopieren",
  copied: "Kopiert",
  inviteLink: "Einladungslink",
  inviteCode: "Einladungscode",
  hide: "Verbergen",
  show: "Anzeigen",
  people: "Personen",
  invite: "Einladen",
  quoted: "„{text}“",
  editingTempo: "die Tempo-Map",
  editingMarkers: "Marker",
  editingProject: "Projekteinstellungen",
  olRememberPos: "HUD-Position merken",
  olShow: "Overlay anzeigen",
  privacy: "Datenschutz",
  help: "Hilfe",
  documentation: "Dokumentation",
  copyDiagnostics: "Diagnose kopieren",
  diagnosticsCopied: "Kopiert",
  installShort: "Installieren",
  updateShort: "Aktualisieren",
  removeShort: "Entfernen",
  resetShort: "Zurücksetzen",
  restartReaperStep: "REAPER neu starten",
  endFollow: "Beenden",
  markerTool: "Marker",
  hostNewerVersion: "Der Host nutzt ein neueres ReaMesh. Aktualisiere, um beizutreten.",
  hostOlderVersion: "Der Host nutzt eine ältere ReaMesh-Version",
  hostOtherVersion: "Dieser Host nutzt eine andere ReaMesh-Version.",
  logVersionMismatch: "Der Host nutzt eine andere ReaMesh-Version.",
  extOldTitle: "REAPER neu starten",
  extOldBody: "ReaMesh wurde aktualisiert. Starte REAPER neu, damit es die passende Erweiterung lädt.",
  appOldTitle: "ReaMesh aktualisieren",
  appOldBody: "In REAPER läuft eine neuere ReaMesh-Erweiterung als diese App.",
  updates: "Updates",
  checkUpdates: "Suchen",
  upToDate: "Auf dem neuesten Stand",
  updateChecking: "Suche nach Updates…",
  updateDownloading: "{version} wird geladen…",
  updateReady: "{version} ist bereit",
  updateReadyAfterSession: "{version} ist bereit. Neustart nach der Session möglich.",
  updateCheckFailed: "Update-Suche fehlgeschlagen",
  restartNow: "Neu starten",
  later: "Später",
  autoCheckUpdates: "Automatisch nach Updates suchen",
  updateReadyTitle: "ReaMesh {version} ist bereit",
  updateReloadHint: "Das Fenster lädt kurz neu.",
  updateAfterSession: "Neustart nach der Session möglich.",
  updateDownloadingTip: "Update {version} wird geladen, {pct} %",
  updateChip: "Update",
  extRestartToLoad: "REAPER neu starten, um {version} zu laden",
  versionLabel: "Version {version}",
  credits: "Danksagungen",
  builtWith: "Gebaut mit",
  allPackages: "Alle Pakete",
  aboutTitle: "Über ReaMesh",
  aboutVersion: "Version",
  aboutBuild: "Build",
  aboutProtocol: "Protokoll",
  releaseBuild: "Release-Build",
  devBuild: "Dev-Build",
  sourceCode: "Quellcode",
  aboutTagline: "Zusammenarbeit in Echtzeit für REAPER, Peer-to-Peer",
  cockosNotice: "REAPER ist eine Marke von Cockos Incorporated. ReaMesh ist unabhängig und nicht mit Cockos verbunden.",
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
  const dict = resolveLang() === "de" ? de : en;
  // A message with a `{count}` may have a singular sibling `<key>One`.
  const one = params && Number(params.count) === 1 ? (`${key}One` as MsgKey) : undefined;
  let s: string = one && one in dict ? dict[one] : dict[key];
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
