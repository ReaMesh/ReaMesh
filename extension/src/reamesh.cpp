// ReaMesh REAPER extension: thin, synchronous capture/apply layer.
//
// Everything in this file runs on REAPER's main thread, driven by the
// registered timer and action hooks. The Rust library (reamesh_ffi.h) owns
// all networking, the CRDT doc and project-file transfers on its own
// background threads; we only exchange JSON strings with it and never block
// on the network.
//
// v3 sync model (owner-authoritative):
//   tracks: index plus the shared scalar fields (name/vol/pan, FX-chain
//            enable, width/pan mode/dual pan/pan law, phase, solo defeat,
//            channel count, color, folder depth, parent send, track timebase,
//            play offset, fixed-lane config), and the track's chunk-level
//            parts (FX chain, input FX chain, automation envelopes, receives
//            with their send envelopes, group/VCA flags, control lock) as
//            normalized state-chunk strings. Solo and mute are deliberately
//            NOT synced: both are per-user monitoring, not document state
//            (their initial values still arrive with the downloaded .rpp).
//   items:  each media item as its full normalized state chunk (covers
//            position, length, splits, fades, takes, take FX, MIDI content)
//            plus the GUID of the track it sits on.
//   project: tempo map, markers/regions, project notes/title/author, track
//            group names and the project timebase as whole fields.
// Media files referenced by an incoming item that are missing locally are
// fetched from the session peers in the background; the item is applied as
// soon as its files exist (deferred apply).
//
// Send status, errors, and activity to the ReaMesh app through the local
// control server instead of writing to the REAPER console. The app is the UI,
// so the extension does not depend on ReaImGui or ReaPack.

#define REAPERAPI_IMPLEMENT
#define REAPERAPI_MINIMAL
#define REAPERAPI_WANT_GetProjectStateChangeCount
#define REAPERAPI_WANT_CountTracks
#define REAPERAPI_WANT_GetTrack
#define REAPERAPI_WANT_GetTrackGUID
#define REAPERAPI_WANT_GetSelectedTrack
#define REAPERAPI_WANT_GetMediaTrackInfo_Value
#define REAPERAPI_WANT_SetMediaTrackInfo_Value
#define REAPERAPI_WANT_GetSetMediaTrackInfo_String
#define REAPERAPI_WANT_InsertTrackAtIndex
#define REAPERAPI_WANT_DeleteTrack
#define REAPERAPI_WANT_PreventUIRefresh
#define REAPERAPI_WANT_guidToString
#define REAPERAPI_WANT_SetOnlyTrackSelected
#define REAPERAPI_WANT_SetTrackSelected
#define REAPERAPI_WANT_CountSelectedTracks
#define REAPERAPI_WANT_ReorderSelectedTracks
#define REAPERAPI_WANT_TrackList_AdjustWindows
#define REAPERAPI_WANT_UpdateArrange
#define REAPERAPI_WANT_CountSelectedMediaItems
#define REAPERAPI_WANT_GetSelectedMediaItem
#define REAPERAPI_WANT_SetMediaItemSelected
#define REAPERAPI_WANT_GetFocusedFX2
#define REAPERAPI_WANT_GetTrackMediaItem
#define REAPERAPI_WANT_GetMediaItemTake
#define REAPERAPI_WANT_TrackFX_GetFloatingWindow
#define REAPERAPI_WANT_TrackFX_Show
#define REAPERAPI_WANT_TrackFX_GetCount
#define REAPERAPI_WANT_TakeFX_GetFloatingWindow
#define REAPERAPI_WANT_TakeFX_Show
#define REAPERAPI_WANT_TakeFX_GetCount
#define REAPERAPI_WANT_Main_OnCommand
#define REAPERAPI_WANT_EnumProjects
#define REAPERAPI_WANT_Main_SaveProject
#define REAPERAPI_WANT_Main_openProject
#define REAPERAPI_WANT_GetResourcePath
#define REAPERAPI_WANT_GetSetObjectState
#define REAPERAPI_WANT_FreeHeapPtr
#define REAPERAPI_WANT_CountMediaItems
#define REAPERAPI_WANT_GetMediaItem
#define REAPERAPI_WANT_GetMediaItem_Track
#define REAPERAPI_WANT_GetSetMediaItemInfo_String
#define REAPERAPI_WANT_AddMediaItemToTrack
#define REAPERAPI_WANT_DeleteTrackMediaItem
#define REAPERAPI_WANT_MoveMediaItemToTrack
#define REAPERAPI_WANT_GetActiveTake
#define REAPERAPI_WANT_GetMediaItemTake_Source
#define REAPERAPI_WANT_GetMediaSourceFileName
#define REAPERAPI_WANT_PCM_Source_CreateFromFile
#define REAPERAPI_WANT_SetMediaItemTake_Source
#define REAPERAPI_WANT_PCM_Source_Destroy
#define REAPERAPI_WANT_UpdateItemInProject
#define REAPERAPI_WANT_ColorToNative
#define REAPERAPI_WANT_ColorFromNative
#define REAPERAPI_WANT_GetSetProjectNotes
#define REAPERAPI_WANT_GetSetProjectInfo_String
#define REAPERAPI_WANT_projectconfig_var_getoffs
#define REAPERAPI_WANT_projectconfig_var_addr
#define REAPERAPI_WANT_CountProjectMarkers
#define REAPERAPI_WANT_EnumProjectMarkers3
#define REAPERAPI_WANT_AddProjectMarker2
#define REAPERAPI_WANT_DeleteProjectMarkerByIndex
#define REAPERAPI_WANT_CountTempoTimeSigMarkers
#define REAPERAPI_WANT_GetTempoTimeSigMarker
#define REAPERAPI_WANT_SetTempoTimeSigMarker
#define REAPERAPI_WANT_DeleteTempoTimeSigMarker
// Live presence (the focused track + timeline position, for the roster and
// click-to-follow), the click-to-follow arrange scroll (GetSet_ArrangeView2),
// plus the local dark-grey item tint that marks media items another peer is
// editing (SetMediaItemInfo_Value writes I_CUSTOMCOLOR). All long-stable APIs,
// so importing them is safe on older REAPER (REAPERAPI_LoadAPI only aborts on a
// missing wanted function).
#define REAPERAPI_WANT_GetMediaItemInfo_Value
#define REAPERAPI_WANT_SetMediaItemInfo_Value
#define REAPERAPI_WANT_GetPlayState
#define REAPERAPI_WANT_GetPlayPosition
#define REAPERAPI_WANT_GetCursorPosition
#define REAPERAPI_WANT_GetSet_ArrangeView2
#define REAPERAPI_WANT_SetEditCurPos
#define REAPERAPI_WANT_format_timestr_pos
#define REAPERAPI_WANT_Undo_OnStateChange
// The minimum-version gate at plugin load, and the host-version mismatch hint.
#define REAPERAPI_WANT_GetAppVersion
// Peak building for media that lands mid-session (processPeakJobs).
#define REAPERAPI_WANT_CountTakes
#define REAPERAPI_WANT_GetMediaSourceParent
#define REAPERAPI_WANT_PCM_Source_BuildPeaks
// The persisted-tint ledger (tintLedgerSave / tintLedgerLine).
#define REAPERAPI_WANT_GetCurrentProjectInLoadSave
#define REAPERAPI_WANT_MarkProjectDirty

#include "reaper_plugin.h"
#include "reaper_plugin_functions.h"

#include "reamesh_ffi.h"
#include "json.hpp"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <filesystem>
#include <fstream>
#include <functional>
#include <iterator>
#include <map>
#include <mutex>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <unordered_set>
#include <thread>
#include <vector>

using nlohmann::json;
namespace fs = std::filesystem;

// ---------- UTF-8 <-> filesystem path ----------
//
// REAPER's API speaks UTF-8 on every platform, but std::filesystem::path
// decodes a *narrow* string with the ANSI code page on Windows: a project in
// ".../Übungen/" turns into mojibake, every fs::exists() against it misses, and
// its media is "missing" forever (items defer, then get dropped). Convert
// explicitly at both boundaries. On POSIX the native encoding is already UTF-8,
// so both are the identity.

static fs::path fsPath(const std::string& utf8) {
#ifdef _WIN32
  return fs::path(std::u8string(reinterpret_cast<const char8_t*>(utf8.data()), utf8.size()));
#else
  return fs::path(utf8);
#endif
}

static std::string fsUtf8(const fs::path& p) {
#ifdef _WIN32
  const std::u8string s = p.u8string();
  return std::string(reinterpret_cast<const char*>(s.data()), s.size());
#else
  return p.string();
#endif
}

// ---------- REAPER version ----------
//
// The oldest REAPER every synced field round-trips on. Every imported function
// exists long before this; what sets the floor is the track attributes read by
// name: fixed lanes (I_NUMFIXEDLANES / C_LANEPLAYS, 7.0), P_LANENAME:n (7.07)
// and C_LANESETTINGS (7.12). An older REAPER answers 0 for an attribute it
// doesn't know and ignores the write, so those fields would silently fail to
// sync (and below 7.0 a v7 .rpp with fixed lanes or FX containers doesn't even
// load faithfully). Rather than degrade quietly, an older REAPER gets the
// extension in a stub mode that only tells the app "update REAPER".
static constexpr const char* kMinReaperVersion = "7.12";

// GetAppVersion() reads "7.42/win64", "7.43+dev0601/macOS-arm64", "7.12rc3/…".
// The part the user knows is everything before the slash.
static std::string reaperVersionLabel(const std::string& appVersion) {
  return appVersion.substr(0, appVersion.find('/'));
}

// REAPER numbers releases as a decimal: 7.0 < 7.07 < 7.12, and 5.11 < 5.111 <
// 5.12. So compare major, then the fraction digits right-padded to three
// ("12" → 120, "07" → 70, "111" → 111). Parsed by hand: strtod would read the
// fraction by the process locale, and a German REAPER has a decimal comma.
// Returns -1 when the string doesn't start with a version.
static long reaperVersionKey(const std::string& v) {
  size_t i = 0;
  long major = 0;
  if (i >= v.size() || v[i] < '0' || v[i] > '9') return -1;
  for (; i < v.size() && v[i] >= '0' && v[i] <= '9'; ++i) major = major * 10 + (v[i] - '0');
  long frac = 0;
  int digits = 0;
  if (i < v.size() && v[i] == '.')
    for (++i; i < v.size() && v[i] >= '0' && v[i] <= '9' && digits < 3; ++i, ++digits)
      frac = frac * 10 + (v[i] - '0');
  for (; digits < 3; ++digits) frac *= 10;
  return major * 1000 + frac;
}

// An unparseable version counts as supported: never lock a user out over a
// version string format we didn't foresee.
static bool reaperVersionSupported(const std::string& appVersion) {
  const long have = reaperVersionKey(appVersion);
  return have < 0 || have >= reaperVersionKey(kMinReaperVersion);
}

// This REAPER's version label ("7.42"), read once at plugin load. Published to
// the app and, while hosting, put into the session meta so a joiner can be
// warned about a mismatch before opening the host's project.
static std::string g_reaperVersion;
// Set at load when this REAPER is older than kMinReaperVersion (or lacks an
// imported function): nothing but the control server runs, and its one state
// frame carries `unsupported` so the app can ask for a REAPER update.
static bool g_unsupported = false;

// ---------- state ----------

struct TrackState {
  int index = 0;
  std::string name;
  double vol = 1.0;
  double pan = 0.0;
  // Solo and mute are deliberately absent: both are per-user *monitoring*
  // state: one peer auditioning (or silencing) a track must not change
  // everyone else's mix. Their initial values still arrive with the
  // downloaded .rpp; they just never sync afterwards.
  bool fxen = true;         // I_FXEN: FX chain enable (the TCP "FX" button)
  double width = 1.0;       // D_WIDTH
  int panmode = -1;         // I_PANMODE
  double dualpanl = -1.0;   // D_DUALPANL (meaningful when panmode == 6)
  double dualpanr = 1.0;    // D_DUALPANR
  double panlaw = -1.0;     // D_PANLAW (<0 = project default)
  int panlawflags = 0;      // I_PANLAW_FLAGS
  bool phase = false;       // B_PHASE
  bool solodefeat = false;  // B_SOLO_DEFEAT
  int nchan = 2;            // I_NCHAN
  int color = 0;            // platform-neutral 0x1RRGGBB, 0 = no custom color
  int folderdepth = 0;      // I_FOLDERDEPTH
  bool mainsend = true;     // B_MAINSEND
  int mainsendoffs = 0;     // C_MAINSEND_OFFS
  int mainsendnch = 0;      // C_MAINSEND_NCH
  int timebase = -1;        // C_BEATATTACHMODE (-1 = project default)
  double playoffs = 0.0;    // D_PLAY_OFFSET
  int playoffsflag = 0;     // I_PLAY_OFFSET_FLAG
  int freemode = 0;         // I_FREEMODE (1 = free positioning, 2 = fixed lanes)
  int numlanes = 0;         // I_NUMFIXEDLANES
  int lanesettings = 0;     // C_LANESETTINGS
  std::string lanenames;    // '\n'-joined P_LANENAME:n (fixed lanes only)
  std::string laneplays;    // ' '-joined C_LANEPLAYS:n (fixed lanes only)
  std::string fxchain;      // normalized <FXCHAIN> block, "" when no FX
  std::string recfx;        // normalized <FXCHAIN_REC> input-FX block
  std::string env;          // concatenated top-level *ENV* blocks (non-send)
  std::string recv;         // AUXRECV lines + send envelopes, source as GUID
  std::string group;        // GROUP_FLAGS / GROUP_FLAGS_HIGH lines
  std::string lock;         // LOCK line (track control lock)
  bool operator==(const TrackState&) const = default;
};

struct ItemState {
  std::string track;  // GUID of the owning track
  std::string chunk;  // normalized item state chunk
  bool operator==(const ItemState&) const = default;
};

// Project-wide state that belongs to neither a track nor an item: the tempo /
// time-signature map and the marker/region list, each serialized to a stable
// line-based string (see captureTempo / captureMarkers).
struct ProjectState {
  std::string tempo;
  std::string markers;
  std::string notes;       // project notes
  std::string title;       // PROJECT_TITLE
  std::string author;      // PROJECT_AUTHOR
  std::string groupnames;  // 64 '\n'-joined TRACK_GROUP_NAME:X entries
  std::string timebase;    // "itemtimelock tempoenvtimelock" config ints
  bool operator==(const ProjectState&) const = default;
};

struct Snapshot {
  std::map<std::string, TrackState> tracks;  // track GUID -> state
  std::map<std::string, ItemState> items;    // item GUID -> state
  ProjectState project;
};

// An incoming item whose media files have not arrived yet. Re-checked every
// tick; the fields are applied once every file exists.
struct DeferredItem {
  std::string guid;
  json fields;
  std::vector<std::string> missing;  // project-relative '/'-separated paths
  std::chrono::steady_clock::time_point since;
};

// An incoming track's receives whose source track doesn't exist here yet.
// AUXRECV crosses the wire with the source track as a GUID, and a unit whose
// source is missing locally has to be dropped from the splice, but the owner
// only re-sends `recv` when the field itself changes, so a dropped unit would
// stay missing for the rest of the session. Park it and retry, like the media
// items above.
struct DeferredRecv {
  std::string wire;  // AUXRECV units in their source-GUID form
  std::chrono::steady_clock::time_point since;
};

enum class Mode { Idle, Hosting, Browsing, Downloading, ClientActive };

static reaper_plugin_info_t* g_rec = nullptr;
static ReameshHandle* g_handle = nullptr;
static Mode g_mode = Mode::Idle;
static std::string g_sessionProjectPath;  // path the session is bound to
static Snapshot g_snapshot;               // last known state, by GUID
static int g_lastChangeCount = -1;
// EMA of what one capture (serialize every track+item chunk + diff) costs on
// this project; drives the adaptive capture/rescan throttle so huge projects
// trade sync latency for a smooth REAPER UI instead of a choppy one.
static std::chrono::milliseconds g_captureCost{0};
static bool g_applyingRemote = false;  // echo-suppression / reentrancy guard
// True only while Main_openProject runs on a finished download; published to
// the app so its download stepper can show the "opening in REAPER" phase.
static bool g_openingProject = false;
static unsigned g_lastPeerCount = 0;
// Media items locally tinted dark grey to signal "another peer is editing this"
// (their editing lock is live, per reamesh_locked_entities). Maps each tinted
// item's GUID -> the REAL native I_CUSTOMCOLOR to restore once the lock
// releases, plus when the tint was (re)applied and whether the safety cap has
// fired. The tint is a PURE LOCAL VIEW TRANSFORM: it recolors the item in
// REAPER but must never enter the sync diff (the snapshot is re-baselined to the
// tinted state after every change) and so is never captured, proposed, or
// written into anyone's .rpp. See reconcileItemTints / restoreAllItemTints.
struct ItemTint {
  int real;                                     // the real I_CUSTOMCOLOR to restore
  std::chrono::steady_clock::time_point since;  // when this tint was (re)applied
  bool expired = false;  // safety cap tripped: shown in real colour, won't re-grey
};
static std::map<std::string, ItemTint> g_itemTints;
// Safety net: a lock tint must never persist longer than this. A live edit keeps
// refreshing the record (applyItem drops it on every remote item update, so the
// next reconcile re-stamps `since`), so this only ever trips on a tint that's
// stuck with no incoming updates: at which point we restore the real colour
// locally. Well clear of a legitimate lock (LOCK_HOLD is 2s; an active editor
// pushes updates ~every second) so it never clears a tint that's genuinely live.
static constexpr std::chrono::seconds TINT_MAX_AGE{8};
// Whether to paint the local lock tint, controlled by the app's set_tint
// command. Disabling the tint leaves editing locks and local snap-back active.
static bool g_tintEnabled = true;
// Last session-ending event, shown by the app as an error toast. Structured
// like a log entry ({code, params, text}); JSON null when there is none.
static json g_lastError;
static std::vector<DeferredItem> g_deferredItems;
// Track GUID -> receives waiting for their source track to show up.
static std::map<std::string, DeferredRecv> g_deferredRecv;
// ---- progressive join ----
// A client opens the downloaded project as soon as the .rpp and the small files
// are here; media keeps downloading behind it. Every item whose media is still
// missing at open time is rewritten to an EMPTY source first (blankMissingSources),
// so REAPER never sees a missing file (no "files not found" dialog, no offline
// items) and the whole arrangement is visible at once, just without waveforms.
// The owner's bootstrap state then re-delivers each such item; missing media
// parks it in g_deferredItems and the real chunk replaces the placeholder as
// soon as its file lands. Until then the placeholder is a PURE LOCAL STAND-IN:
// pinned out of the capture diff (pinPlaceholderItems), tinted grey like a
// peer-locked item, and guarded twice more (guardMediaBlanking here, and the
// Rust push/admit checks) so an empty source can never travel to the owner and
// wipe media out of an item there.
static std::set<std::string> g_placeholders;  // item GUIDs still standing in for media
// True while this client is still receiving the project it joined: the
// background download is running, or placeholders are still waiting for their
// media. Published as state.loading; local edits snap back meanwhile
// (revertWhileLoading) and editing unlocks once everything is here.
static bool g_mediaLoading = false;
// Mirror of status.background (the Rust download still streaming media).
static bool g_downloadBackground = false;
// After the download finished, placeholders get this long to resolve through
// the deferred-item path before we stop holding the editing gate for them.
// They stay pinned out of the diff either way (see pinPlaceholderItems).
static constexpr std::chrono::seconds PLACEHOLDER_GRACE{20};
static std::chrono::steady_clock::time_point g_placeholderGraceSince{};
static bool g_placeholdersGivenUp = false;
static std::chrono::steady_clock::time_point g_lastCapture{};
static std::chrono::steady_clock::time_point g_lastFullScan{};
// Client only: cumulative count of own proposals the owner overrode, as
// reported by reamesh_status ("reverts"); deltas become activity-log events.
static long g_lastReverts = 0;
// True once it is safe to propose local edits: always for the owner, and for a
// client only after the owner's first authoritative state has been applied and
// re-baselined. Until then a client must stay silent: opening the .rpp and
// REAPER's own load side effects must never be diffed against a not-yet-synced
// baseline and shipped as a burst of phantom proposals the owner rejects (the
// old "everything snapped back the moment I joined" bug).
static bool g_bootstrapSynced = false;
static std::chrono::steady_clock::time_point g_clientActiveSince{};

// "Join without effects": the user chose the trust dialog's second button, so
// this client opens a copy of the downloaded project with every FX chain
// (track, input, take, master) stripped, and stays STRICTLY read-only for the
// session: it never captures or proposes, and it drops fx fields (and take-FX
// blocks) from incoming applies. The read-only part is what makes the mode
// safe: a client that proposed anything from a stripped project would sooner
// or later push a chunk with the host's plugins missing and wipe their work.
static bool g_noFx = false;
// Pre-host warning ({files,bytes,free,cloud,enough} or null): hosting would
// copy this much external media into the project folder (and/or the project
// lives in a cloud-synced folder). The app confirms before the host proceeds.
static json g_hostWarning;
// Post-download plugin verification ({extras:[...],listed} or null): the
// received .rpp loads plugins the host's manifest meta didn't list. The open
// is held until the app confirms (open_confirm) or the user leaves.
static json g_pendingVerify;
static std::string g_pendingOpenPath;  // project path held back for that answer
// Follow mode: peer id whose transport/view this REAPER mirrors ("" = none).
static std::string g_followPeer;
static std::string g_followLastFocus;   // last focus GUID acted on
static double g_followLastPlay = -1.0;  // last beacon position acted on
static double g_followV0 = 0.0, g_followV1 = 0.0;  // last adopted view span
static std::string g_followLastSel;     // last mirrored item selection (joined GUIDs)
static std::string g_followLastFx;      // last mirrored FX-window descriptor (dumped JSON)
// The FX window follow mode opened on this REAPER, so it can be closed (or
// switched) when the followed peer moves on. Only what follow mode itself
// opened is ever closed: the user's own windows are theirs.
struct FollowFx {
  std::string track;  // track GUID
  std::string item;   // item GUID for a take FX, "" for a track FX
  int take = 0;       // take index (take FX)
  int fx = 0;         // FX index (raw REAPER index: may carry the input-FX / container bits)
  bool flt = false;   // floating window (else the chain window)
  bool valid = false;
};
static FollowFx g_followFxOpen;
// Presence detail this user beacons for followers: the selected item GUIDs and
// the FX window being edited in. Sampled at most every 100ms (the fast beacon
// cadence) because a selection walk isn't free on a big project and the beacon
// leaves no faster than that anyway.
static constexpr size_t kPresenceSelMax = 256;
static json g_presSel = json::array();
static json g_presFx;  // null = no FX window open
static std::chrono::steady_clock::time_point g_presSampled{};
// Last re-seek of a rolling transport (steady_clock), so drift correction
// can never turn into a stutter: re-seeks are audible, one per second at most.
static std::chrono::steady_clock::time_point g_followLastSeek{};
// Follow-mode drift tolerance (seconds) between our play position and the
// followed peer's estimated one before we re-seek. Beacons arrive every ~100ms
// while we follow and their age + half the RTT is compensated, so the estimate
// is good to well under 100ms; the tolerance stays generous because a re-seek
// is an audible glitch and the two REAPERs free-run at the same rate anyway.
static constexpr double kFollowDriftSec = 0.5;
// Hosting: mtime of the served .rpp when the session meta (track count +
// plugin list) was last computed; a mid-session save changes the served file,
// so the meta follows it.
static fs::file_time_type g_metaRppMtime{};
static std::chrono::steady_clock::time_point g_lastMetaCheck{};

// Defined in the control-server section below; used before that point.
static std::string jsonStr(const json& j, const char* key);
static bool jsonBool(const json& j, const char* key, bool dflt);
static double jsonNum(const json& j, const char* key, double dflt);
static json handleJson(char* (*fn)(ReameshHandle*), const json& fallback);
static void openDownloadedProject(std::string path);

// In-flight automatic rejoin after the host vanished without a farewell
// (network blip / crash). While set, the Browsing tick watches the lobby for
// the same session id and rejoins with the remembered passphrase.
struct Reconnect {
  std::string sessionId;
  std::string sessionName;
  std::string passphrase;
  // Invite ticket the session was joined through (empty for LAN-discovered
  // sessions). A ticket host is usually NOT on this LAN, so waiting for its
  // lobby announce would wait forever: the rejoin re-dials the ticket instead.
  std::string ticket;
  std::chrono::steady_clock::time_point deadline;
};
static std::optional<Reconnect> g_reconnect;
// Passphrase the current session was joined with, kept for auto-rejoin.
static std::string g_joinPassphrase;
// Invite ticket the current session was joined with ("" = discovered on the
// LAN), kept for auto-rejoin like the passphrase.
static std::string g_joinTicket;

// Two parameter-free convenience actions; the ReaMesh app is the real UI
// and drives everything (browse/join/leave) over the control server instead.
static int g_cmdHost = 0, g_cmdLeave = 0;
static custom_action_register_t g_caHost = {
    0, "REAMESH_HOST", "ReaMesh: Host session (share current project on LAN)", nullptr};
static custom_action_register_t g_caLeave = {
    0, "REAMESH_LEAVE", "ReaMesh: Leave session / stop hosting", nullptr};

// Activity log streamed to the ReaMesh app (never the REAPER console).
// Entries are structured ({"code", "params", "text"}) so the app can render
// them in the user's language; `text` is the English fallback for codes the
// app doesn't know (or a raw message when `code` is empty).
static std::deque<json> g_log;

static json logEvent(const char* code, json params, const std::string& text) {
  json e = {{"code", code}, {"params", std::move(params)}, {"text", text}};
  g_log.push_back(e);
  while (g_log.size() > 80) g_log.pop_front();
  return e;
}

// Structural activity events ("a track was added", "tempo changed", …) shown as
// the overlay's short feed. Two guards keep it signal, not noise: a mute window
// swallows the burst when a session first opens (the whole project arrives as
// one changeset), and identical events de-duplicate within a few seconds (one
// drag re-emits its field repeatedly). Join/leave and override notices call
// logEvent directly: those are never muted.
static std::chrono::steady_clock::time_point g_activityMuteUntil;
static std::deque<std::pair<std::string, std::chrono::steady_clock::time_point>> g_recentActivity;

static void muteActivity(std::chrono::seconds d) {
  g_activityMuteUntil = std::chrono::steady_clock::now() + d;
}

static void activityEvent(const char* code, json params, const std::string& text) {
  const auto now = std::chrono::steady_clock::now();
  if (now < g_activityMuteUntil) return;
  // The owner's whole project lands as one big changeset right after a client
  // connects; never narrate that settle as a flurry of "a track was added"
  // notices. The time mute above covers the burst's tail; this covers a slow
  // apply that outruns it (bootstrap flips true on the first applied state).
  if (g_mode == Mode::ClientActive && !g_bootstrapSynced) return;
  const std::string key = std::string(code) + '|' + text;
  for (const auto& [k, when] : g_recentActivity)
    if (k == key && now - when < std::chrono::seconds(3)) return;
  g_recentActivity.push_back({key, now});
  while (g_recentActivity.size() > 16) g_recentActivity.pop_front();
  logEvent(code, std::move(params), text);
}

// ---------- state-chunk utilities ----------

// Full state chunk of a track or media item (RPP-syntax text).
static std::string objectStateChunk(void* obj) {
  char* raw = GetSetObjectState(obj, "");
  if (!raw) return "";
  std::string out(raw);
  FreeHeapPtr(raw);
  return out;
}

static void setObjectStateChunk(void* obj, const std::string& chunk) {
  char* ret = GetSetObjectState(obj, chunk.c_str());
  if (ret) FreeHeapPtr(ret);  // non-null only for the getter path, but be safe
}

static std::string_view ltrim(std::string_view s) {
  while (!s.empty() && (s.front() == ' ' || s.front() == '\t')) s.remove_prefix(1);
  return s;
}

// Volatile per-user UI state inside chunks: selection, which FX window is
// open, dock state, floating window rects. Synced verbatim they would make
// peers fight over selection and pop each other's plugin windows open, and
// they bump the diff on every click. Envelope ARM and VIS lines are in the
// same bucket: which envelope is armed/visible is the local user's automation
// workflow, not document state (REAPER falls back to defaults when an applied
// envelope block carries neither).
//
// Every prefix here ends in a space (or is matched whole): chunks carry base64
// plugin-state lines, and base64's alphabet includes upper-case letters but
// never a space, so an unanchored prefix like "FLOAT" would also swallow a
// payload line that happens to begin "FLOATxxxx…", silently truncating the FX
// state that then goes on the wire. (A currently floating FX window is written
// as `FLOAT x y w h`, a closed one as `FLOATPOS x y w h`: both per-user.)
static bool isVolatileChunkLine(std::string_view trimmed) {
  return trimmed.rfind("SEL ", 0) == 0 || trimmed.rfind("SHOW ", 0) == 0 ||
         trimmed.rfind("LASTSEL ", 0) == 0 || trimmed.rfind("DOCKED ", 0) == 0 ||
         trimmed.rfind("FLOATPOS ", 0) == 0 || trimmed.rfind("FLOAT ", 0) == 0 ||
         trimmed == "FLOAT" ||
         trimmed.rfind("WNDRECT ", 0) == 0 ||
         trimmed.rfind("ARM ", 0) == 0 || trimmed.rfind("VIS ", 0) == 0;
}

// REAPER writes media paths with the platform's separator, so an item captured
// on Windows carries `FILE "Media\take.wav"`. macOS cannot resolve that at all
// on POSIX a backslash is an ordinary filename character, not a separator:
// so every item from a Windows host would land with dead media. Forward
// slashes work on both, so that is what goes on the wire. Windows REAPER
// re-serializes them back to backslashes locally, and this normalizes them
// away again on the next capture, so the round trip stays byte-stable.
//
// Only the quoted path is touched: the rest of a chunk (base64 FX state in
// particular) must survive verbatim.
static std::string normalizeFilePathLine(std::string_view line) {
  std::string out(line);
  const size_t q1 = out.find('"');
  const size_t q2 = out.rfind('"');
  if (q1 == std::string::npos || q2 <= q1) return out;
  std::replace(out.begin() + q1 + 1, out.begin() + q2, '\\', '/');
  return out;
}

static std::string normalizeChunk(const std::string& chunk) {
  std::string out;
  out.reserve(chunk.size());
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    std::string_view line(chunk.data() + pos, eol - pos);
    // REAPER serializes chunks with the platform's line ending: the very same
    // track reads back CRLF on Windows and LF on macOS. Left alone, every
    // chunk-carried field would look changed to a peer on the other platform
    // and the two would overwrite each other forever. LF-only on the wire:
    // REAPER's parser accepts either on apply, so this round-trips stably.
    if (!line.empty() && line.back() == '\r') line.remove_suffix(1);
    const std::string_view trimmed = ltrim(line);
    if (!isVolatileChunkLine(trimmed)) {
      if (trimmed.rfind("FILE \"", 0) == 0)
        out.append(normalizeFilePathLine(line));
      else
        out.append(line);
      out.push_back('\n');
    }
    pos = eol + 1;
  }
  return out;
}

// The synced top-level parts of a track chunk:
//   <FXCHAIN>      the track FX chain
//   <FXCHAIN_REC>  the input (record) FX chain
//   *ENV* blocks   automation envelopes, except AUX*ENV send envelopes,
//                  which belong to the AUXRECV receive they follow and ride
//                  in `recv` so the pairing survives splicing
//   AUXRECV lines  receives (with their AUX*ENV send-envelope blocks)
//   GROUP_FLAGS*   grouping/VCA membership bitmasks
//   LOCK           track control lock
// Items (<ITEM>) deliberately stay out: they are synced per-item through the
// "items" section.
static bool isEnvTag(std::string_view tag) {
  return tag.find("ENV") != std::string_view::npos;
}

struct TrackBlocks {
  std::string fxchain;
  std::string recfx;
  std::string env;
  std::string recv;
  std::string group;
  std::string lock;
};

// Part kinds, in the order they are emitted when several land at the same
// insertion point (roughly REAPER's own chunk order; the parser is
// order-tolerant).
enum TrackPartKind { kLock = 0, kGroup, kRecv, kRecFx, kFxChain, kEnv, kPartCount };

static std::string_view blockTag(std::string_view trimmed) {
  if (trimmed.empty() || trimmed.front() != '<') return {};
  std::string_view tag = trimmed.substr(1);
  return tag.substr(0, tag.find_first_of(" \t"));
}

// Classify a depth-1 line of a track chunk; `tag` is the block tag when the
// line opens a block, empty otherwise. -1 for everything that isn't a synced
// part (scalar lines, <ITEM> blocks, ...).
static int trackPartKind(std::string_view trimmed, std::string_view tag) {
  if (!tag.empty()) {
    if (tag == "FXCHAIN") return kFxChain;
    if (tag == "FXCHAIN_REC") return kRecFx;
    if (isEnvTag(tag)) return tag.rfind("AUX", 0) == 0 ? kRecv : kEnv;
    return -1;
  }
  if (trimmed.rfind("AUXRECV ", 0) == 0) return kRecv;
  if (trimmed.rfind("GROUP_FLAGS", 0) == 0) return kGroup;  // + GROUP_FLAGS_HIGH
  if (trimmed.rfind("LOCK ", 0) == 0) return kLock;
  return -1;
}

// Extract the synced parts from a (normalized) track chunk, in document order
// within each part (receives keep their send envelopes interleaved).
static TrackBlocks extractTrackBlocks(const std::string& chunk) {
  TrackBlocks out;
  std::string* dest[kPartCount] = {&out.lock,  &out.group,   &out.recv,
                                   &out.recfx, &out.fxchain, &out.env};
  int depth = 0;
  int collectKind = -1;  // >= 0 while inside a collected block
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    std::string_view line(chunk.data() + pos, eol - pos);
    std::string_view t = ltrim(line);
    bool opens = !t.empty() && t.front() == '<';
    if (collectKind < 0 && depth == 1 && !t.empty()) {
      int k = trackPartKind(t, blockTag(t));
      if (k >= 0) {
        if (opens) {
          collectKind = k;  // the block, opener included, collects below
        } else {
          dest[k]->append(line);
          dest[k]->push_back('\n');
        }
      }
    }
    if (collectKind >= 0) {
      dest[collectKind]->append(line);
      dest[collectKind]->push_back('\n');
    }
    if (opens) {
      ++depth;
    } else if (t == ">") {
      --depth;
      if (collectKind >= 0 && depth == 1) collectKind = -1;
    }
    pos = eol + 1;
  }
  return out;
}

// Patch for replaceTrackBlocks: non-null members replace that part of the
// chunk (empty string = remove that part), null members are left alone.
struct TrackBlocksPatch {
  const std::string* fxchain = nullptr;
  const std::string* recfx = nullptr;
  const std::string* env = nullptr;
  const std::string* recv = nullptr;  // local (track-index) AUXRECV form
  const std::string* group = nullptr;
  const std::string* lock = nullptr;
};

// Rebuild a track chunk with the patched parts replaced. Each replacement
// lands where the first old segment of its kind was, else before the first
// item, else before the closing ">".
static std::string replaceTrackBlocks(const std::string& chunk, const TrackBlocksPatch& p) {
  const std::string* repl[kPartCount] = {p.lock, p.group, p.recv, p.recfx, p.fxchain, p.env};

  std::vector<std::string> lines;
  {
    size_t pos = 0;
    while (pos < chunk.size()) {
      size_t eol = chunk.find('\n', pos);
      if (eol == std::string::npos) eol = chunk.size();
      lines.emplace_back(chunk.substr(pos, eol - pos));
      pos = eol + 1;
    }
  }

  std::vector<std::string> out;
  out.reserve(lines.size());
  int depth = 0;
  int skipUntilDepth = -1;  // inside a block being removed
  size_t anchor[kPartCount];
  for (size_t& a : anchor) a = std::string::npos;
  size_t firstItemAt = std::string::npos;

  for (const std::string& line : lines) {
    std::string_view t = ltrim(line);
    bool opens = !t.empty() && t.front() == '<';
    bool closes = (t == ">");

    if (skipUntilDepth < 0 && depth == 1 && !t.empty()) {
      std::string_view tag = blockTag(t);
      int k = trackPartKind(t, tag);
      if (k >= 0 && repl[k]) {
        if (anchor[k] == std::string::npos) anchor[k] = out.size();
        if (opens) {
          skipUntilDepth = depth;  // skip until the block closes
          ++depth;
        }
        continue;  // line dropped / block skipped
      }
      if (opens && tag == "ITEM" && firstItemAt == std::string::npos)
        firstItemAt = out.size();
    }

    if (opens) {
      ++depth;
    } else if (closes) {
      --depth;
      if (skipUntilDepth >= 0 && depth == skipUntilDepth) {
        skipUntilDepth = -1;
        continue;
      }
    }
    if (skipUntilDepth >= 0) continue;
    out.push_back(line);
  }

  const size_t fallback =
      (firstItemAt != std::string::npos) ? firstItemAt : (out.empty() ? 0 : out.size() - 1);
  std::vector<std::vector<int>> insAt(out.size() + 1);
  for (int k = 0; k < kPartCount; ++k) {
    if (!repl[k]) continue;
    size_t a = (anchor[k] != std::string::npos) ? anchor[k] : fallback;
    if (a > out.size()) a = out.size();
    insAt[a].push_back(k);  // ascending k = canonical order at equal spots
  }

  std::string result;
  result.reserve(chunk.size() + 256);
  for (size_t i = 0; i <= out.size(); ++i) {
    for (int k : insAt[i]) {
      if (repl[k]->empty()) continue;
      result += *repl[k];  // parts end in '\n' by construction
      if (result.back() != '\n') result.push_back('\n');
    }
    if (i < out.size()) {
      result += out[i];
      result.push_back('\n');
    }
  }
  return result;
}

// Receives cross the wire with the source-track GUID in place of the AUXRECV
// line's leading track index: indices are per-peer, GUIDs are the shared
// identity, and the GUID form is stable across reorders, so a track move
// doesn't re-emit every receive as an edit. `mapEndpoint` converts the
// leading token; returning "" drops the whole unit (the AUXRECV line plus the
// AUX*ENV send-envelope blocks that follow it), used when the source track
// doesn't exist on this side.
static std::string transformRecvUnits(
    const std::string& recv, const std::function<std::string(const std::string&)>& mapEndpoint) {
  std::string out;
  bool dropping = false;
  int depth = 0;
  size_t pos = 0;
  while (pos < recv.size()) {
    size_t eol = recv.find('\n', pos);
    if (eol == std::string::npos) eol = recv.size();
    std::string_view line(recv.data() + pos, eol - pos);
    std::string_view t = ltrim(line);
    if (depth == 0 && t.rfind("AUXRECV ", 0) == 0) {
      std::string_view rest = t.substr(8);
      size_t sp = rest.find_first_of(" \t");
      std::string tok(rest.substr(0, sp == std::string_view::npos ? rest.size() : sp));
      std::string mapped = mapEndpoint(tok);
      dropping = mapped.empty();
      if (!dropping) {
        out.append(line.substr(0, line.size() - t.size()));  // keep indentation
        out += "AUXRECV ";
        out += mapped;
        out.append(rest.substr(tok.size()));
        out.push_back('\n');
      }
    } else if (!dropping) {
      out.append(line);
      out.push_back('\n');
    }
    if (!t.empty() && t.front() == '<') {
      ++depth;
    } else if (t == ">") {
      --depth;
    }
    pos = eol + 1;
  }
  return out;
}

// All quoted FILE references in an item chunk (media sources).
static std::string rppQuotedToken(std::string_view line, size_t from);
static int colorToWire(int native);
static int colorFromWire(int wire);

static std::vector<std::string> chunkFilePaths(const std::string& chunk) {
  std::vector<std::string> out;
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    std::string_view t = ltrim(std::string_view(chunk.data() + pos, eol - pos));
    if (t.rfind("FILE ", 0) == 0) {
      // REAPER quotes a path with ", or with ' / ` when the path itself
      // contains quotes: rppQuotedToken understands all three.
      std::string path = rppQuotedToken(t, 5);
      if (!path.empty()) out.push_back(std::move(path));
    }
    pos = eol + 1;
  }
  return out;
}

static bool isRelativePath(const std::string& p) {
  if (p.empty() || p.front() == '/' || p.front() == '\\') return false;
  if (p.size() >= 2 && isalpha((unsigned char)p[0]) && p[1] == ':') return false;
  return true;
}

// `path` relative to `dir` ('/'-separated) when it lies strictly inside it,
// else "". Purely lexical: both come from REAPER (the project path and the
// paths it resolved against it), so they share their spelling. Separators
// compare equal either way; drive-letter paths compare case-insensitively.
static std::string pathInsideDir(std::string path, std::string dir) {
  std::replace(path.begin(), path.end(), '\\', '/');
  std::replace(dir.begin(), dir.end(), '\\', '/');
  while (dir.size() > 1 && dir.back() == '/') dir.pop_back();
  if (dir.empty() || isRelativePath(dir) || path.size() <= dir.size() + 1) return "";
  if (path[dir.size()] != '/') return "";
  const bool drive = dir.size() >= 2 && dir[1] == ':';
  for (size_t i = 0; i < dir.size(); ++i) {
    const char a = path[i], b = dir[i];
    if (a == b) continue;
    if (!drive || tolower((unsigned char)a) != tolower((unsigned char)b)) return "";
  }
  std::string rel = path.substr(dir.size() + 1);
  // A ".." component would climb back out; "." components are noise REAPER
  // never writes. Either way: not a clean in-project path, leave it alone.
  size_t start = 0;
  while (start <= rel.size()) {
    size_t end = rel.find('/', start);
    if (end == std::string::npos) end = rel.size();
    const std::string_view part(rel.data() + start, end - start);
    if (part.empty() || part == "." || part == "..") return "";
    start = end + 1;
  }
  return rel;
}

// REAPER serializes an item's media as an ABSOLUTE path (`FILE
// "/Users/ben/Song/Audio/kick.wav"`), whatever the .rpp says. Sent like that, a
// peer on the same machine (two REAPER instances, or a joined copy next to the
// original) resolves it to the OTHER peer's file, which exists there: its take
// then reads from outside its own project, internalizeExternalMedia copies the
// file into its project root and repoints, its next edit sends that absolute
// path back, and the other side does the same. Every in-project source ended
// up duplicated in the project root. On the wire media paths are therefore
// project-relative: each REAPER resolves `Audio/kick.wav` against its own
// project folder (and writes it back absolute, which this maps again, so the
// round trip stays byte-stable). Paths outside every `dirs` entry are left
// as they are. Only double-quoted FILE paths are rewritten; REAPER uses ' or `
// only for a path that itself contains a double quote, which stays absolute.
static std::string relativizeChunkFiles(const std::string& chunk,
                                        const std::vector<std::string>& dirs) {
  std::string out;
  out.reserve(chunk.size());
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    const bool hasEol = eol != std::string::npos;
    if (!hasEol) eol = chunk.size();
    const std::string_view line(chunk.data() + pos, eol - pos);
    const std::string_view t = ltrim(line);
    std::string rel;
    size_t q1 = std::string_view::npos, q2 = std::string_view::npos;
    if (t.rfind("FILE \"", 0) == 0) {
      q1 = line.find('"');
      q2 = line.find('"', q1 + 1);
      if (q2 != std::string_view::npos) {
        const std::string path(line.substr(q1 + 1, q2 - q1 - 1));
        for (const std::string& d : dirs)
          if (!(rel = pathInsideDir(path, d)).empty()) break;
      }
    }
    if (rel.empty()) {
      out.append(line);
    } else {
      out.append(line.substr(0, q1 + 1));
      out.append(rel);
      out.append(line.substr(q2));
    }
    if (hasEol) out.push_back('\n');
    pos = eol + 1;
  }
  return out;
}

// The inverse, applied to every incoming item chunk before REAPER sees it:
// each project-relative double-quoted FILE path becomes `dir`/path, in the
// separator `dir` is spelled with. Leaving the resolution to REAPER failed on
// a real project: the host's folder was "Reaper Projekte " (trailing space),
// a guest's newly added sound arrived as "Whoosh.wav", REAPER resolved it
// somewhere else, the item stayed offline and the project saved a path to a
// file that doesn't exist. The file the fetch put at projectDir()/path is
// exactly what this names. Recapture maps the absolute path back to the same
// relative text, so the round trip stays byte-stable. Paths that climb out
// ("..") or are already absolute stay as they are; so does everything when
// `dir` is empty (no saved project).
static std::string absolutizeChunkFiles(const std::string& chunk, std::string dir) {
  if (dir.empty() || isRelativePath(dir)) return chunk;
  const bool backslash = dir.find('\\') != std::string::npos && dir.find('/') == std::string::npos;
  const char sep = backslash ? '\\' : '/';
  while (dir.size() > 1 && (dir.back() == '/' || dir.back() == '\\')) dir.pop_back();
  std::string out;
  out.reserve(chunk.size() + 256);
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    const bool hasEol = eol != std::string::npos;
    if (!hasEol) eol = chunk.size();
    const std::string_view line(chunk.data() + pos, eol - pos);
    const std::string_view t = ltrim(line);
    std::string abs;
    size_t q1 = std::string_view::npos, q2 = std::string_view::npos;
    if (t.rfind("FILE \"", 0) == 0) {
      q1 = line.find('"');
      q2 = line.find('"', q1 + 1);
      if (q2 != std::string_view::npos) {
        std::string rel(line.substr(q1 + 1, q2 - q1 - 1));
        std::replace(rel.begin(), rel.end(), '\\', '/');
        // Same cleanliness rule as pathInsideDir: no empty, "." or ".." part.
        bool clean = isRelativePath(rel);
        for (size_t start = 0; clean && start <= rel.size();) {
          size_t end = rel.find('/', start);
          if (end == std::string::npos) end = rel.size();
          const std::string_view part(rel.data() + start, end - start);
          clean = !part.empty() && part != "." && part != "..";
          start = end + 1;
        }
        if (clean) {
          if (backslash) std::replace(rel.begin(), rel.end(), '/', '\\');
          abs = dir + sep + rel;
        }
      }
    }
    if (abs.empty()) {
      out.append(line);
    } else {
      out.append(line.substr(0, q1 + 1));
      out.append(abs);
      out.append(line.substr(q2));
    }
    if (hasEol) out.push_back('\n');
    pos = eol + 1;
  }
  return out;
}

// ---------- plugin inventory / FX stripping (pure text helpers) ----------
//
// Both sides of the join-preview story parse RPP text with the same helper:
// the host inventories the .rpp it serves (the manifest's "this project loads
// X, Y, Z" claim), and the client re-inventories the .rpp it actually received
// before opening it, so a dishonest host's list is caught, not trusted.
// Unit-tested by tools/chunk_test.cpp.

// The first quoted token after `from` in an RPP line. RPP quotes with ", ' or
// ` depending on what the payload itself contains.
static std::string rppQuotedToken(std::string_view line, size_t from) {
  for (size_t i = from; i < line.size(); ++i) {
    const char q = line[i];
    if (q == '"' || q == '\'' || q == '`') {
      const size_t close = line.find(q, i + 1);
      if (close == std::string_view::npos) return "";
      return std::string(line.substr(i + 1, close - i - 1));
    }
    if (line[i] != ' ' && line[i] != '\t') return "";  // unquoted payload
  }
  return "";
}

// Every plugin an RPP-syntax text would instantiate, in document order,
// de-duplicated: the display name of each <VST/<CLAP/<AU/<DX/<LV2 block
// (e.g. "VST3: Serum (Xfer Records)") and "JS: <path>" for JS effects. Works
// on whole project files and on single chunks/blocks alike.
static std::vector<std::string> collectPlugins(const std::string& text) {
  std::vector<std::string> out;
  std::set<std::string> seen;
  auto push = [&](std::string name) {
    if (!name.empty() && seen.insert(name).second) out.push_back(std::move(name));
  };
  size_t pos = 0;
  while (pos < text.size()) {
    size_t eol = text.find('\n', pos);
    if (eol == std::string::npos) eol = text.size();
    std::string_view line(text.data() + pos, eol - pos);
    if (!line.empty() && line.back() == '\r') line.remove_suffix(1);
    std::string_view t = ltrim(line);
    if (!t.empty() && t.front() == '<') {
      const std::string_view tag = blockTag(t);
      if (tag == "VST" || tag == "CLAP" || tag == "AU" || tag == "DX" || tag == "LV2") {
        push(rppQuotedToken(t, 1 + tag.size()));
      } else if (tag == "JS") {
        // JS lines carry the effect path, quoted only when it has spaces.
        std::string_view rest = ltrim(t.substr(1 + tag.size()));
        std::string name;
        if (!rest.empty() && (rest.front() == '"' || rest.front() == '\'' || rest.front() == '`')) {
          name = rppQuotedToken(rest, 0);
        } else {
          const size_t sp = rest.find_first_of(" \t");
          name = std::string(rest.substr(0, sp == std::string_view::npos ? rest.size() : sp));
        }
        if (!name.empty()) push("JS: " + name);
      }
    }
    pos = eol + 1;
  }
  return out;
}

// Remove every FX-carrying block from an RPP-syntax text, at any depth:
// <FXCHAIN> (track FX), <FXCHAIN_REC> (input FX), <TAKEFX> (per-take FX) and
// <MASTERFXLIST> (the master chain in a project file). With `takeFxOnly` only
// <TAKEFX> goes: that is the item-chunk variant used by the no-FX apply path.
// Output is LF-normalized (REAPER's parser accepts either ending).
static std::string stripFxBlocks(const std::string& text, bool takeFxOnly) {
  std::string out;
  out.reserve(text.size());
  int depth = 0;
  int skipUntilDepth = -1;
  size_t pos = 0;
  while (pos < text.size()) {
    size_t eol = text.find('\n', pos);
    if (eol == std::string::npos) eol = text.size();
    std::string_view line(text.data() + pos, eol - pos);
    if (!line.empty() && line.back() == '\r') line.remove_suffix(1);
    std::string_view t = ltrim(line);
    const bool opens = !t.empty() && t.front() == '<';
    const bool closes = (t == ">");
    if (skipUntilDepth < 0 && opens) {
      const std::string_view tag = blockTag(t);
      const bool strip =
          tag == "TAKEFX" ||
          (!takeFxOnly && (tag == "FXCHAIN" || tag == "FXCHAIN_REC" || tag == "MASTERFXLIST"));
      if (strip) {
        skipUntilDepth = depth;
        ++depth;
        pos = eol + 1;
        continue;
      }
    }
    if (opens) {
      ++depth;
    } else if (closes) {
      --depth;
      if (skipUntilDepth >= 0 && depth == skipUntilDepth) {
        skipUntilDepth = -1;
        pos = eol + 1;
        continue;
      }
    }
    if (skipUntilDepth < 0) {
      out.append(line);
      out.push_back('\n');
    }
    pos = eol + 1;
  }
  return out;
}

// ---------- progressive join: placeholder rewrite (pure text helper) ----------
//
// Rewrite every take source in an RPP-syntax text whose media is not present
// (per `present(path)`, path as written in the FILE line) to `<SOURCE EMPTY>`,
// keeping the item itself (position, length, name, fades, color, takes) intact.
// The result is a project REAPER opens without any missing file. Returns the
// GUIDs (IGUID) of the items that were rewritten. Output is LF-normalized.
// Unit-tested by tools/chunk_test.cpp.
struct BlankResult {
  std::string text;
  std::set<std::string> guids;
};

static BlankResult blankMissingSources(const std::string& text,
                                       const std::function<bool(const std::string&)>& present) {
  BlankResult r;
  r.text.reserve(text.size());
  // Block stack: depth and, for each open block, whether it is an <ITEM and
  // (for the outermost <SOURCE of a take) whether we are buffering it.
  struct Open { std::string tag; };
  std::vector<Open> stack;
  int itemDepth = -1;             // stack size at which the current item opened
  std::string itemGuid;           // IGUID of the current item
  std::vector<std::string> itemGuidStack;
  // The outermost <SOURCE block directly under an item is buffered whole, then
  // emitted verbatim or replaced.
  std::string sourceBuf;
  size_t sourceDepth = 0;         // stack size at which that source opened (0 = not buffering)
  std::string sourceIndent;
  size_t pos = 0;
  while (pos < text.size()) {
    size_t eol = text.find('\n', pos);
    if (eol == std::string::npos) eol = text.size();
    std::string_view line(text.data() + pos, eol - pos);
    if (!line.empty() && line.back() == '\r') line.remove_suffix(1);
    std::string_view t = ltrim(line);
    const bool opens = !t.empty() && t.front() == '<';
    const bool closes = (t == ">");
    pos = eol + 1;

    if (sourceDepth) {
      sourceBuf.append(line);
      sourceBuf.push_back('\n');
      if (opens) {
        stack.push_back({std::string(blockTag(t))});
      } else if (closes) {
        stack.pop_back();
        if (stack.size() + 1 == sourceDepth) {
          // The take's whole source is buffered: keep it, or blank it.
          bool missing = false;
          for (std::string f : chunkFilePaths(sourceBuf)) {
            std::replace(f.begin(), f.end(), '\\', '/');
            if (!present(f)) { missing = true; break; }
          }
          if (missing) {
            r.text += sourceIndent + "<SOURCE EMPTY\n" + sourceIndent + ">\n";
            if (!itemGuid.empty()) r.guids.insert(itemGuid);
          } else {
            r.text += sourceBuf;
          }
          sourceBuf.clear();
          sourceDepth = 0;
        }
      }
      continue;
    }

    if (opens) {
      const std::string tag(blockTag(t));
      if (tag == "SOURCE" && itemDepth >= 0 && (int)stack.size() == itemDepth + 1) {
        // Outermost source of a take (a SECTION source nests its inner WAVE
        // source, which the buffered walk below swallows along with it).
        sourceDepth = stack.size() + 1;
        sourceIndent.assign(line.substr(0, line.size() - t.size()));
        sourceBuf.assign(line);
        sourceBuf.push_back('\n');
        stack.push_back({tag});
        continue;
      }
      if (tag == "ITEM") {
        itemGuidStack.push_back(itemGuid);
        itemDepth = (int)stack.size();
        itemGuid.clear();
      }
      stack.push_back({tag});
    } else if (closes) {
      if (!stack.empty()) {
        const bool closingItem = stack.back().tag == "ITEM";
        stack.pop_back();
        if (closingItem) {
          itemGuid = itemGuidStack.empty() ? std::string() : itemGuidStack.back();
          if (!itemGuidStack.empty()) itemGuidStack.pop_back();
          // Back to the enclosing item, if any (items don't nest, but be safe).
          itemDepth = -1;
          for (int i = (int)stack.size() - 1; i >= 0; --i)
            if (stack[i].tag == "ITEM") { itemDepth = i; break; }
        }
      }
    } else if (itemDepth >= 0 && (int)stack.size() == itemDepth + 1 && t.rfind("IGUID ", 0) == 0) {
      std::string_view g = ltrim(t.substr(6));
      itemGuid.assign(g.substr(0, g.find_first_of(" \t")));
    }
    r.text.append(line);
    r.text.push_back('\n');
  }
  // Unterminated source at EOF (malformed text): emit what we have verbatim.
  r.text += sourceBuf;
  return r;
}

// Does an item chunk carry a file-backed source? (MIDI, empty and section-less
// items don't.) Used by the placeholder bookkeeping and the media-blanking guard.
static bool chunkHasFileSource(const std::string& chunk) {
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    if (ltrim(std::string_view(chunk.data() + pos, eol - pos)).rfind("FILE ", 0) == 0) return true;
    pos = eol + 1;
  }
  return false;
}

// Whether an item chunk sets the item's own colour: a `COLOR` line directly
// inside `<ITEM` (nested blocks are skipped). REAPER writes none for an item in
// the default colour, and applying such a chunk leaves whatever colour the item
// had, so the caller has to reset it explicitly.
static bool itemChunkHasColor(const std::string& chunk) {
  int depth = 0;
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    std::string_view t = ltrim(std::string_view(chunk.data() + pos, eol - pos));
    if (!t.empty() && t.front() == '<') {
      ++depth;
    } else if (!t.empty() && t.front() == '>') {
      --depth;
    } else if (depth == 1 && t.rfind("COLOR ", 0) == 0) {
      return true;
    }
    pos = eol + 1;
  }
  return false;
}

static bool chunkHasEmptySource(const std::string& chunk) {
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    if (ltrim(std::string_view(chunk.data() + pos, eol - pos)).rfind("<SOURCE EMPTY", 0) == 0)
      return true;
    pos = eol + 1;
  }
  return false;
}

// ---------- shared (non-synced) flag markers ----------
//
// A marker whose name starts with this prefix is a *shared flag*: what the
// roster bar's shared markers used to drop into every project (now pings, see
// sharePing; an older peer still inserts one when pinged). Deliberately
// outside the synced marker list: captureMarkers skips them (they never ride the wire) and applyMarkers
// leaves them standing (a remote marker rebuild deletes only synced ones), so
// every person can keep or delete their copy independently. (Renaming a
// normal marker to start with "⚑ " opts it out of sync: by design.)
static constexpr const char kSharedMarkerPrefix[] = "\xE2\x9A\x91 ";  // "⚑ "

static bool isSharedMarkerName(const char* name) {
  return name && strncmp(name, kSharedMarkerPrefix, sizeof(kSharedMarkerPrefix) - 1) == 0;
}

// ---------- snapshot & diff (capture side) ----------

static std::string trackGuid(MediaTrack* tr) {
  char buf[64] = "";
  guidToString(GetTrackGUID(tr), buf);
  return buf;
}

static MediaTrack* findTrackByGuid(const std::string& guid) {
  const int n = CountTracks(nullptr);
  for (int i = 0; i < n; ++i) {
    MediaTrack* tr = GetTrack(nullptr, i);
    if (trackGuid(tr) == guid) return tr;
  }
  return nullptr;
}

// Human-readable name for a track GUID, or "" if it's unknown or unnamed. Used
// by the override notice and activity feed so the app can say *which* track,
// not just that something changed.
static std::string trackNameByGuid(const std::string& guid) {
  MediaTrack* tr = findTrackByGuid(guid);
  if (!tr) return "";
  char buf[512] = "";
  if (GetSetMediaTrackInfo_String(tr, "P_NAME", buf, false) && buf[0]) return buf;
  return "";
}

// Display name of a peer (the editing-lock holder), resolved from the roster by
// peer id. Empty when the id is unknown or the peer never sent a name beacon.
static std::string peerNameById(const json& peers, const std::string& id) {
  if (id.empty() || !peers.is_array()) return "";
  for (const auto& p : peers)
    if (p.is_object() && jsonStr(p, "id") == id)
      return jsonStr(p, "name");
  return "";
}

static std::string itemGuid(MediaItem* item) {
  char buf[64] = "";
  GetSetMediaItemInfo_String(item, "GUID", buf, false);
  return buf;
}

static MediaItem* findItemByGuid(const std::string& guid) {
  const int n = CountMediaItems(nullptr);
  for (int i = 0; i < n; ++i) {
    MediaItem* item = GetMediaItem(nullptr, i);
    if (itemGuid(item) == guid) return item;
  }
  return nullptr;
}

static fs::path projectDir() {
  return fsPath(g_sessionProjectPath).parent_path();
}

// ---------- project-wide state: tempo map + markers/regions ----------

// One line per tempo/time-signature marker: "timepos bpm num denom linear".
// Position is absolute seconds so it reproduces without depending on the rest
// of the map; points are re-added in order.
static std::string captureTempo() {
  std::string out;
  const int n = CountTempoTimeSigMarkers(nullptr);
  for (int i = 0; i < n; ++i) {
    double timepos = 0, beatpos = 0, bpm = 0;
    int measurepos = 0, num = 0, den = 0;
    bool linear = false;
    if (!GetTempoTimeSigMarker(nullptr, i, &timepos, &measurepos, &beatpos, &bpm, &num, &den,
                               &linear))
      continue;
    char line[160];
    // Full double precision so capture->apply->recapture round-trips exactly
    // and a re-applied map never re-emits as a phantom edit.
    snprintf(line, sizeof(line), "%.17g %.17g %d %d %d\n", timepos, bpm, num, den, linear ? 1 : 0);
    out += line;
  }
  return out;
}

static void applyTempo(const std::string& s) {
  for (int i = CountTempoTimeSigMarkers(nullptr) - 1; i >= 0; --i)
    DeleteTempoTimeSigMarker(nullptr, i);
  size_t pos = 0;
  while (pos < s.size()) {
    size_t eol = s.find('\n', pos);
    if (eol == std::string::npos) eol = s.size();
    std::string line = s.substr(pos, eol - pos);
    pos = eol + 1;
    double timepos = 0, bpm = 0;
    int num = 0, den = 0, linear = 0;
    if (sscanf(line.c_str(), "%lf %lf %d %d %d", &timepos, &bpm, &num, &den, &linear) == 5)
      SetTempoTimeSigMarker(nullptr, -1, timepos, -1, -1, bpm, num, den, linear != 0);
  }
}

// One line per marker/region: "M|R \t index \t pos \t rgnend \t color \t name".
// Name is last so it may contain spaces; tabs/newlines are stripped on capture.
static std::string sanitizeMarkerName(const char* name) {
  std::string out = name ? name : "";
  for (char& c : out)
    if (c == '\t' || c == '\n' || c == '\r') c = ' ';
  return out;
}

static std::string captureMarkers() {
  std::string out;
  int idx = 0, next;
  bool isrgn = false;
  double pos = 0, rgnend = 0;
  const char* name = nullptr;
  int num = 0, color = 0;
  while ((next = EnumProjectMarkers3(nullptr, idx, &isrgn, &pos, &rgnend, &name, &num, &color)) !=
         0) {
    // Shared flag markers are local by design: never on the wire.
    if (isSharedMarkerName(name)) {
      idx = next;
      continue;
    }
    char buf[160];
    // Colors cross the wire platform-neutral, like track colors: macOS and
    // Windows pack native ints with opposite byte order.
    snprintf(buf, sizeof(buf), "%s\t%d\t%.17g\t%.17g\t%d\t", isrgn ? "R" : "M", num, pos, rgnend,
             colorToWire(color));
    out += buf;
    out += sanitizeMarkerName(name);
    out += '\n';
    idx = next;
  }
  return out;
}

static void applyMarkers(const std::string& s) {
  // Rebuild only the *synced* markers: shared flag markers stay standing:
  // each person keeps or deletes their own copy (capture filters them out, so
  // the wire list never contains one to re-add either).
  std::vector<int> doomed;
  {
    int idx = 0, next;
    bool isrgn = false;
    double pos = 0, rgnend = 0;
    const char* name = nullptr;
    int num = 0, color = 0;
    while ((next = EnumProjectMarkers3(nullptr, idx, &isrgn, &pos, &rgnend, &name, &num,
                                       &color)) != 0) {
      if (!isSharedMarkerName(name)) doomed.push_back(idx);
      idx = next;
    }
  }
  for (auto it = doomed.rbegin(); it != doomed.rend(); ++it)
    DeleteProjectMarkerByIndex(nullptr, *it);

  size_t pos = 0;
  while (pos < s.size()) {
    size_t eol = s.find('\n', pos);
    if (eol == std::string::npos) eol = s.size();
    std::string line = s.substr(pos, eol - pos);
    pos = eol + 1;
    if (line.empty()) continue;
    // Split the five leading tab fields; the name is whatever remains.
    std::vector<std::string> f;
    size_t p = 0;
    for (int k = 0; k < 5; ++k) {
      size_t tab = line.find('\t', p);
      if (tab == std::string::npos) { f.clear(); break; }
      f.push_back(line.substr(p, tab - p));
      p = tab + 1;
    }
    if (f.size() != 5) continue;
    std::string name = line.substr(p);
    bool isrgn = f[0] == "R";
    int num = atoi(f[1].c_str());
    double mpos = strtod(f[2].c_str(), nullptr);
    double rgnend = strtod(f[3].c_str(), nullptr);
    int color = atoi(f[4].c_str());
    AddProjectMarker2(nullptr, isrgn, mpos, rgnend, name.c_str(), num, colorFromWire(color));
  }
}

// Track colors cross the wire as platform-neutral 0x1RRGGBB (the 0x1000000
// "custom color in use" flag plus RGB); macOS and Windows pack native colors
// with opposite byte order, so the native int must never be shared.
static int colorToWire(int native) {
  if (!(native & 0x1000000)) return 0;
  int r = 0, g = 0, b = 0;
  ColorFromNative(native & 0xFFFFFF, &r, &g, &b);
  return 0x1000000 | (r << 16) | (g << 8) | b;
}

static int colorFromWire(int wire) {
  if (!(wire & 0x1000000)) return 0;
  return 0x1000000 | ColorToNative((wire >> 16) & 0xFF, (wire >> 8) & 0xFF, wire & 0xFF);
}

// Pointer to a 4-byte project config var, or null when REAPER doesn't know
// it. Used for the project timebase, which has no setter API
// (GetSetProjectInfo's PROJECT_TIMEBASE is read-only).
static int* projConfigInt(const char* name) {
  int sz = 0;
  const int offs = projectconfig_var_getoffs(name, &sz);
  if (offs < 0 || sz != (int)sizeof(int)) return nullptr;
  return (int*)projectconfig_var_addr(nullptr, offs);
}

// "itemtimelock tempoenvtimelock": project timebase for items/envelopes/
// markers and for the tempo map. Synced because tempo edits ARE synced: with
// diverging timebases the same tempo change moves items differently per peer.
static std::string captureProjectTimebase() {
  int* item = projConfigInt("itemtimelock");
  int* tempo = projConfigInt("tempoenvtimelock");
  if (!item || !tempo) return "";
  return std::to_string(*item) + " " + std::to_string(*tempo);
}

static void applyProjectTimebase(const std::string& s) {
  int it = 0, te = 0;
  if (sscanf(s.c_str(), "%d %d", &it, &te) != 2) return;
  if (int* item = projConfigInt("itemtimelock")) *item = it;
  if (int* tempo = projConfigInt("tempoenvtimelock")) *tempo = te;
}

static std::string captureProjectString(const char* desc) {
  char buf[4096] = "";
  GetSetProjectInfo_String(nullptr, desc, buf, false);
  return buf;
}

static void applyProjectString(const char* desc, const std::string& value) {
  std::vector<char> buf(value.begin(), value.end());
  buf.push_back('\0');
  GetSetProjectInfo_String(nullptr, desc, buf.data(), true);
}

static std::string captureProjectNotes() {
  std::vector<char> buf(1 << 16, '\0');
  GetSetProjectNotes(nullptr, false, buf.data(), (int)buf.size());
  // LF-only on the wire, for the same reason as normalizeChunk: notes come
  // back CRLF on Windows and LF on macOS, and the raw difference alone would
  // read as an edit every tick between peers on different platforms.
  std::string notes = buf.data();
  notes.erase(std::remove(notes.begin(), notes.end(), '\r'), notes.end());
  return notes;
}

static void applyProjectNotes(const std::string& notes) {
  std::vector<char> buf(notes.begin(), notes.end());
  buf.push_back('\0');
  GetSetProjectNotes(nullptr, true, buf.data(), 0);
}

// All 64 track-group names as one '\n'-joined field (names can't contain
// newlines; the Project Settings field is single-line).
//
// Reading them is 64 GetSetProjectInfo_String round trips, and
// snapshotProject() runs several times a second (capture, remote apply, tint
// reconcile, deferred flush): far too often for a field that changes maybe
// once a session. Cache it against the project change counter (an undoable
// edit invalidates it) AND let it age out on the rescan cadence: the counter
// is only ever a latency optimization here (see captureLocalChanges: the 2s
// rescan exists precisely for edits that don't bump it), so a rename made
// through a path that adds no undo point must still be picked up within the
// same window it always was. applyGroupNames invalidates explicitly for the
// remote-apply path.
static std::string g_groupNamesCache;
static int g_groupNamesAt = -1;
static std::chrono::steady_clock::time_point g_groupNamesWhen{};
static constexpr std::chrono::seconds GROUP_NAMES_MAX_AGE{2};

static std::string captureGroupNames() {
  const int cc = GetProjectStateChangeCount(nullptr);
  const auto now = std::chrono::steady_clock::now();
  if (cc == g_groupNamesAt && now - g_groupNamesWhen < GROUP_NAMES_MAX_AGE)
    return g_groupNamesCache;
  std::string out;
  for (int i = 1; i <= 64; ++i) {
    char desc[32];
    snprintf(desc, sizeof(desc), "TRACK_GROUP_NAME:%d", i);
    if (i > 1) out += '\n';
    out += captureProjectString(desc);
  }
  g_groupNamesAt = cc;
  g_groupNamesWhen = now;
  g_groupNamesCache = out;
  return out;
}

static void applyGroupNames(const std::string& s) {
  // Writing project info does not reliably bump the change counter, so drop
  // the cache by hand or the next capture would re-read the stale value.
  g_groupNamesAt = -1;
  size_t pos = 0;
  for (int i = 1; i <= 64 && pos <= s.size(); ++i) {
    size_t eol = s.find('\n', pos);
    if (eol == std::string::npos) eol = s.size();
    char desc[32];
    snprintf(desc, sizeof(desc), "TRACK_GROUP_NAME:%d", i);
    applyProjectString(desc, s.substr(pos, eol - pos));
    pos = eol + 1;
  }
}

// The folder(s) an item's media paths are made relative to on capture (see
// relativizeChunkFiles): the active project's folder as REAPER spells it, plus
// its canonical form when that differs (a project opened through a symlink).
// Taken from the project being snapshotted, not g_sessionProjectPath: a client
// takes its baseline snapshot before that is set. Cached per project path.
static const std::vector<std::string>& mediaPathDirs() {
  static std::string forPath;
  static std::vector<std::string> dirs;
  char fn[4096] = "";
  EnumProjects(-1, fn, sizeof(fn));
  if (forPath != fn) {
    forPath = fn;
    dirs.clear();
    if (fn[0]) {
      const fs::path dir = fsPath(fn).parent_path();
      dirs.push_back(fsUtf8(dir));
      std::error_code ec;
      const fs::path canon = fs::weakly_canonical(dir, ec);
      if (!ec && !canon.empty() && fsUtf8(canon) != dirs.front()) dirs.push_back(fsUtf8(canon));
    }
  }
  return dirs;
}

static Snapshot snapshotProject() {
  Snapshot snap;
  snap.project.tempo = captureTempo();
  snap.project.markers = captureMarkers();
  snap.project.notes = captureProjectNotes();
  snap.project.title = captureProjectString("PROJECT_TITLE");
  snap.project.author = captureProjectString("PROJECT_AUTHOR");
  snap.project.groupnames = captureGroupNames();
  snap.project.timebase = captureProjectTimebase();
  const int n = CountTracks(nullptr);
  for (int i = 0; i < n; ++i) {
    MediaTrack* tr = GetTrack(nullptr, i);
    TrackState ts;
    ts.index = i;
    char name[512] = "";
    GetSetMediaTrackInfo_String(tr, "P_NAME", name, false);
    ts.name = name;
    ts.vol = GetMediaTrackInfo_Value(tr, "D_VOL");
    ts.pan = GetMediaTrackInfo_Value(tr, "D_PAN");
    ts.fxen = GetMediaTrackInfo_Value(tr, "I_FXEN") != 0.0;
    ts.width = GetMediaTrackInfo_Value(tr, "D_WIDTH");
    ts.panmode = (int)GetMediaTrackInfo_Value(tr, "I_PANMODE");
    ts.dualpanl = GetMediaTrackInfo_Value(tr, "D_DUALPANL");
    ts.dualpanr = GetMediaTrackInfo_Value(tr, "D_DUALPANR");
    ts.panlaw = GetMediaTrackInfo_Value(tr, "D_PANLAW");
    ts.panlawflags = (int)GetMediaTrackInfo_Value(tr, "I_PANLAW_FLAGS");
    ts.phase = GetMediaTrackInfo_Value(tr, "B_PHASE") != 0.0;
    ts.solodefeat = GetMediaTrackInfo_Value(tr, "B_SOLO_DEFEAT") != 0.0;
    ts.nchan = (int)GetMediaTrackInfo_Value(tr, "I_NCHAN");
    ts.color = colorToWire((int)GetMediaTrackInfo_Value(tr, "I_CUSTOMCOLOR"));
    ts.folderdepth = (int)GetMediaTrackInfo_Value(tr, "I_FOLDERDEPTH");
    ts.mainsend = GetMediaTrackInfo_Value(tr, "B_MAINSEND") != 0.0;
    ts.mainsendoffs = (int)GetMediaTrackInfo_Value(tr, "C_MAINSEND_OFFS");
    ts.mainsendnch = (int)GetMediaTrackInfo_Value(tr, "C_MAINSEND_NCH");
    ts.timebase = (int)GetMediaTrackInfo_Value(tr, "C_BEATATTACHMODE");
    ts.playoffs = GetMediaTrackInfo_Value(tr, "D_PLAY_OFFSET");
    ts.playoffsflag = (int)GetMediaTrackInfo_Value(tr, "I_PLAY_OFFSET_FLAG");
    ts.freemode = (int)GetMediaTrackInfo_Value(tr, "I_FREEMODE");
    ts.numlanes = (int)GetMediaTrackInfo_Value(tr, "I_NUMFIXEDLANES");
    ts.lanesettings = (int)GetMediaTrackInfo_Value(tr, "C_LANESETTINGS");
    if (ts.freemode == 2 && ts.numlanes > 0) {
      for (int lane = 0; lane < ts.numlanes; ++lane) {
        char key[32], lname[512] = "";
        snprintf(key, sizeof(key), "P_LANENAME:%d", lane);
        GetSetMediaTrackInfo_String(tr, key, lname, false);
        if (lane) ts.lanenames += '\n';
        ts.lanenames += lname;
        snprintf(key, sizeof(key), "C_LANEPLAYS:%d", lane);
        if (lane) ts.laneplays += ' ';
        ts.laneplays += std::to_string((int)GetMediaTrackInfo_Value(tr, key));
      }
    }
    TrackBlocks blocks = extractTrackBlocks(normalizeChunk(objectStateChunk(tr)));
    ts.fxchain = std::move(blocks.fxchain);
    ts.recfx = std::move(blocks.recfx);
    ts.env = std::move(blocks.env);
    ts.group = std::move(blocks.group);
    ts.lock = std::move(blocks.lock);
    ts.recv = transformRecvUnits(blocks.recv, [](const std::string& tok) -> std::string {
      char* end = nullptr;
      const long idx = strtol(tok.c_str(), &end, 10);
      if (end == tok.c_str() || idx < 0) return "";
      MediaTrack* src = GetTrack(nullptr, (int)idx);
      return src ? trackGuid(src) : "";
    });
    snap.tracks[trackGuid(tr)] = std::move(ts);
  }
  const std::vector<std::string>& mediaDirs = mediaPathDirs();
  const int ni = CountMediaItems(nullptr);
  for (int i = 0; i < ni; ++i) {
    MediaItem* item = GetMediaItem(nullptr, i);
    ItemState is;
    is.track = trackGuid(GetMediaItem_Track(item));
    is.chunk = relativizeChunkFiles(normalizeChunk(objectStateChunk(item)), mediaDirs);
    snap.items[itemGuid(item)] = std::move(is);
  }
  return snap;
}

static json trackToJson(const TrackState& ts) {
  return json{{"present", true},
              {"index", ts.index},
              {"name", ts.name},
              {"vol", ts.vol},
              {"pan", ts.pan},
              {"fxen", ts.fxen},
              {"width", ts.width},
              {"panmode", ts.panmode},
              {"dualpanl", ts.dualpanl},
              {"dualpanr", ts.dualpanr},
              {"panlaw", ts.panlaw},
              {"panlawflags", ts.panlawflags},
              {"phase", ts.phase},
              {"solodefeat", ts.solodefeat},
              {"nchan", ts.nchan},
              {"color", ts.color},
              {"folderdepth", ts.folderdepth},
              {"mainsend", ts.mainsend},
              {"mainsendoffs", ts.mainsendoffs},
              {"mainsendnch", ts.mainsendnch},
              {"timebase", ts.timebase},
              {"playoffs", ts.playoffs},
              {"playoffsflag", ts.playoffsflag},
              {"freemode", ts.freemode},
              {"numlanes", ts.numlanes},
              {"lanesettings", ts.lanesettings},
              {"lanenames", ts.lanenames},
              {"laneplays", ts.laneplays},
              {"fxchain", ts.fxchain},
              {"recfx", ts.recfx},
              {"env", ts.env},
              {"recv", ts.recv},
              {"group", ts.group},
              {"lock", ts.lock}};
}

// Minimal changeset: full field set for new entities, changed fields only for
// known ones, a present:false tombstone for removed ones. Empty string when
// nothing relevant changed.
static std::string diffSnapshots(const Snapshot& prev, const Snapshot& now) {
  json tracks = json::object();
  for (const auto& [guid, ts] : now.tracks) {
    auto it = prev.tracks.find(guid);
    if (it == prev.tracks.end()) {
      tracks[guid] = trackToJson(ts);
      continue;
    }
    const TrackState& old = it->second;
    if (old == ts) continue;
    json delta = json::object();
    if (old.index != ts.index) delta["index"] = ts.index;
    if (old.name != ts.name) delta["name"] = ts.name;
    if (old.vol != ts.vol) delta["vol"] = ts.vol;
    if (old.pan != ts.pan) delta["pan"] = ts.pan;
    if (old.fxen != ts.fxen) delta["fxen"] = ts.fxen;
    if (old.width != ts.width) delta["width"] = ts.width;
    if (old.panmode != ts.panmode) delta["panmode"] = ts.panmode;
    if (old.dualpanl != ts.dualpanl) delta["dualpanl"] = ts.dualpanl;
    if (old.dualpanr != ts.dualpanr) delta["dualpanr"] = ts.dualpanr;
    if (old.panlaw != ts.panlaw) delta["panlaw"] = ts.panlaw;
    if (old.panlawflags != ts.panlawflags) delta["panlawflags"] = ts.panlawflags;
    if (old.phase != ts.phase) delta["phase"] = ts.phase;
    if (old.solodefeat != ts.solodefeat) delta["solodefeat"] = ts.solodefeat;
    if (old.nchan != ts.nchan) delta["nchan"] = ts.nchan;
    if (old.color != ts.color) delta["color"] = ts.color;
    if (old.folderdepth != ts.folderdepth) delta["folderdepth"] = ts.folderdepth;
    if (old.mainsend != ts.mainsend) delta["mainsend"] = ts.mainsend;
    if (old.mainsendoffs != ts.mainsendoffs) delta["mainsendoffs"] = ts.mainsendoffs;
    if (old.mainsendnch != ts.mainsendnch) delta["mainsendnch"] = ts.mainsendnch;
    if (old.timebase != ts.timebase) delta["timebase"] = ts.timebase;
    if (old.playoffs != ts.playoffs) delta["playoffs"] = ts.playoffs;
    if (old.playoffsflag != ts.playoffsflag) delta["playoffsflag"] = ts.playoffsflag;
    if (old.freemode != ts.freemode) delta["freemode"] = ts.freemode;
    if (old.numlanes != ts.numlanes) delta["numlanes"] = ts.numlanes;
    if (old.lanesettings != ts.lanesettings) delta["lanesettings"] = ts.lanesettings;
    if (old.lanenames != ts.lanenames) delta["lanenames"] = ts.lanenames;
    if (old.laneplays != ts.laneplays) delta["laneplays"] = ts.laneplays;
    if (old.fxchain != ts.fxchain) delta["fxchain"] = ts.fxchain;
    if (old.recfx != ts.recfx) delta["recfx"] = ts.recfx;
    if (old.env != ts.env) delta["env"] = ts.env;
    if (old.recv != ts.recv) delta["recv"] = ts.recv;
    if (old.group != ts.group) delta["group"] = ts.group;
    if (old.lock != ts.lock) delta["lock"] = ts.lock;
    if (!delta.empty()) tracks[guid] = delta;
  }
  for (const auto& [guid, ts] : prev.tracks) {
    if (!now.tracks.count(guid)) tracks[guid] = json{{"present", false}};
  }

  json items = json::object();
  for (const auto& [guid, is] : now.items) {
    auto it = prev.items.find(guid);
    if (it == prev.items.end()) {
      items[guid] = json{{"present", true}, {"track", is.track}, {"chunk", is.chunk}};
      continue;
    }
    const ItemState& old = it->second;
    if (old == is) continue;
    json delta = json::object();
    if (old.track != is.track) delta["track"] = is.track;
    if (old.chunk != is.chunk) delta["chunk"] = is.chunk;
    items[guid] = delta;
  }
  for (const auto& [guid, is] : prev.items) {
    if (!now.items.count(guid)) items[guid] = json{{"present", false}};
  }

  // Project-wide state lives under section "project", single id "project".
  json project = json::object();
  {
    json delta = json::object();
    if (prev.project.tempo != now.project.tempo) delta["tempo"] = now.project.tempo;
    if (prev.project.markers != now.project.markers) delta["markers"] = now.project.markers;
    if (prev.project.notes != now.project.notes) delta["notes"] = now.project.notes;
    if (prev.project.title != now.project.title) delta["title"] = now.project.title;
    if (prev.project.author != now.project.author) delta["author"] = now.project.author;
    if (prev.project.groupnames != now.project.groupnames)
      delta["groupnames"] = now.project.groupnames;
    if (prev.project.timebase != now.project.timebase) delta["timebase"] = now.project.timebase;
    if (!delta.empty()) project["project"] = delta;
  }

  json changeset = json::object();
  if (!tracks.empty()) changeset["tracks"] = tracks;
  if (!items.empty()) changeset["items"] = items;
  if (!project.empty()) changeset["project"] = project;
  if (changeset.empty()) return "";
  return changeset.dump();
}

// Media imported from outside the project folder cannot be served to peers
// (only project-relative paths cross the wire). Copy such sources into the
// project folder and repoint the take, like REAPER's own "copy media into
// project" option. Runs during capture, so the path change is published.
// Background media-internalization plumbing. Copying an out-of-project source
// (a 500 MB WAV from another disk) used to run synchronously in the capture,
// freezing REAPER's main thread for the whole copy. Copies now run on detached
// worker threads writing to a hidden ".reamesh-copy-*" temp (dotfiles are
// excluded from the transfer manifest, so a half-copied file is never served)
// renamed into place when whole; results are drained, and takes repointed:
// on the next capture, on the main thread. While a source is still copying,
// its items are pinned out of the capture diff (see captureLocalChanges), so
// an absolute external path never reaches the wire.
struct CopyResult {
  std::string src, dest, err;
  bool ok;
};
static std::mutex g_copyMx;
static std::vector<CopyResult> g_copyDone;      // filled by worker threads
static std::set<std::string> g_copyInFlight;    // source paths, main thread only
static std::set<std::string> g_copyFailed;      // gave up (logged once)
// item GUID -> external source path, rebuilt each capture: which items must
// stay out of the diff because their media is still on its way in.
static std::map<std::string, std::string> g_itemsAwaitingCopy;

// Defined below, with processPeakJobs.
static void queuePeakBuild(const std::string& guid);

// Repoint every take still reading `srcPath` to the in-project copy at `dest`.
// Peaks are keyed by source path, so a repointed take starts without them:
// each one goes to the same sliced peak builder fetched media uses. REAPER's
// own "build any missing peaks" ran on the main thread right as the sound was
// added, and the arrange didn't repaint when it finished.
static bool repointTakes(const std::string& srcPath, const std::string& dest) {
  bool repointed = false;
  const int n = CountMediaItems(nullptr);
  for (int i = 0; i < n; ++i) {
    MediaItem* item = GetMediaItem(nullptr, i);
    MediaItem_Take* take = GetActiveTake(item);
    if (!take) continue;
    PCM_source* src = GetMediaItemTake_Source(take);
    if (!src) continue;
    char fn[4096] = "";
    GetMediaSourceFileName(src, fn, sizeof(fn));
    if (srcPath != fn) continue;
    PCM_source* ns = PCM_Source_CreateFromFile(dest.c_str());
    if (!ns) continue;
    SetMediaItemTake_Source(take, ns);
    PCM_Source_Destroy(src);
    UpdateItemInProject(item);
    queuePeakBuild(itemGuid(item));
    repointed = true;
  }
  return repointed;
}

static void internalizeExternalMedia() {
  const fs::path projDir = projectDir();
  if (projDir.empty()) return;
  bool repointed = false;

  // Drain finished background copies first: repoint their takes so this very
  // capture picks the in-project path up as a normal local edit.
  std::vector<CopyResult> done;
  {
    std::lock_guard<std::mutex> lk(g_copyMx);
    done.swap(g_copyDone);
  }
  for (const CopyResult& r : done) {
    g_copyInFlight.erase(r.src);
    const std::string name = fsUtf8(fsPath(r.src).filename());
    if (!r.ok) {
      g_copyFailed.insert(r.src);  // don't retry (and re-log) every capture
      logEvent("media_copy_failed", {{"file", name}},
               "could not copy " + name + " into the project: " + r.err);
      continue;
    }
    repointed |= repointTakes(r.src, r.dest);
    logEvent("media_copied", {{"file", name}},
             "copied " + name + " into the project folder so peers get it");
  }

  // Scan for external sources and kick off copies for new ones.
  g_itemsAwaitingCopy.clear();
  const int n = CountMediaItems(nullptr);
  for (int i = 0; i < n; ++i) {
    MediaItem* item = GetMediaItem(nullptr, i);
    MediaItem_Take* take = GetActiveTake(item);
    if (!take) continue;
    PCM_source* src = GetMediaItemTake_Source(take);
    if (!src) continue;
    char fn[4096] = "";
    GetMediaSourceFileName(src, fn, sizeof(fn));
    if (!fn[0]) continue;  // in-project MIDI etc.
    std::error_code ec;
    fs::path source = fsPath(fn);
    fs::path rel = fs::relative(source, projDir, ec);
    // A leading ".." component means the source lives outside the project
    // folder. Compare the first *component* rather than a string prefix:
    // path::native() is wide on Windows, and a directory literally named
    // "..something" must not read as "outside".
    const bool outside = !rel.empty() && *rel.begin() == fs::path("..");
    if (!ec && !rel.empty() && !outside) continue;  // already inside
    if (g_copyFailed.count(fn)) continue;
    if (g_copyInFlight.count(fn)) {
      g_itemsAwaitingCopy[itemGuid(item)] = fn;  // still copying: keep it off the wire
      continue;
    }

    fs::path dest = projDir / source.filename();
    for (int k = 2; fs::exists(dest, ec) && fs::file_size(dest, ec) != fs::file_size(source, ec);
         ++k)
      dest = projDir / fsPath(fsUtf8(source.stem()) + " (" + std::to_string(k) + ")" +
                              fsUtf8(source.extension()));
    if (fs::exists(dest, ec)) {
      // Identical-size copy already in the project (a prior run): repoint now.
      repointed |= repointTakes(fn, fsUtf8(dest));
      continue;
    }

    g_copyInFlight.insert(fn);
    g_itemsAwaitingCopy[itemGuid(item)] = fn;
    // srcPath/destPath stay UTF-8: they are compared against REAPER's own take
    // filenames and reported back to the app. Only the fs calls get converted.
    std::thread([srcPath = fsUtf8(source), destPath = fsUtf8(dest)] {
      std::error_code tec;
      const fs::path destination = fsPath(destPath);
      fs::path tmp = destination.parent_path() /
                     fsPath(".reamesh-copy-" + fsUtf8(destination.filename()));
      fs::copy_file(fsPath(srcPath), tmp, fs::copy_options::overwrite_existing, tec);
      bool ok = !tec;
      std::string err = tec ? tec.message() : "";
      if (ok) {
        fs::rename(tmp, destination, tec);
        ok = !tec;
        if (!ok) err = tec.message();
      }
      if (!ok) fs::remove(tmp, tec);
      std::lock_guard<std::mutex> lk(g_copyMx);
      g_copyDone.push_back({srcPath, destPath, err, ok});
    }).detach();
  }

  if (repointed) UpdateArrange();
}

// Defined below, near the tint helpers (needs applyItem / lockedItemsByOthers).
static void revertLockedItemEdits(Snapshot& snap);
// Defined below, after applyChangeset.
static void revertWhileLoading(const json& changeset, const Snapshot& base);

// Progressive-join placeholders never enter the diff, in either direction: a
// standing placeholder is pinned to its baseline (no chunk/track delta, and no
// tombstone if the user deleted it locally), and one the baseline doesn't know
// is dropped from the snapshot (never proposed as a creation).
static void pinPlaceholderItems(Snapshot& snap) {
  for (const std::string& guid : g_placeholders) {
    auto base = g_snapshot.items.find(guid);
    if (base == g_snapshot.items.end()) {
      snap.items.erase(guid);
      continue;
    }
    snap.items[guid] = base->second;
  }
}

// Last local line of defence before a changeset leaves this REAPER: no item
// chunk may replace a file-backed source with an empty one (that is exactly
// what a leaked placeholder looks like, and no REAPER editing gesture produces
// it), and nothing about a standing placeholder is proposed at all. Judged
// against `base`, the pre-capture baseline. Returns the filtered changeset
// ("" when nothing is left). The Rust side repeats the check on both ends.
static std::string guardMediaBlanking(const std::string& changeset, const Snapshot& base) {
  json cs = json::parse(changeset, nullptr, false);
  if (!cs.is_object() || !cs.contains("items") || !cs["items"].is_object()) return changeset;
  json& items = cs["items"];
  bool blocked = false;
  for (auto it = items.begin(); it != items.end();) {
    const std::string& guid = it.key();
    json& fields = it.value();
    bool drop = g_placeholders.count(guid) > 0;
    if (!drop && fields.is_object() && fields.contains("chunk") && fields["chunk"].is_string()) {
      const std::string& chunk = fields["chunk"].get_ref<const std::string&>();
      auto b = base.items.find(guid);
      const bool hadFile = b != base.items.end() && chunkHasFileSource(b->second.chunk);
      if (hadFile && chunkHasEmptySource(chunk) && !chunkHasFileSource(chunk)) {
        fields.erase("chunk");
        blocked = true;
        drop = fields.empty();
      }
    }
    if (drop) {
      blocked = true;
      it = items.erase(it);
    } else {
      ++it;
    }
  }
  if (!blocked) return changeset;
  activityEvent("placeholder_blocked", json::object(),
                "kept an item that is still waiting for its media out of the sync");
  if (items.empty()) cs.erase("items");
  return cs.empty() ? std::string() : cs.dump();
}

static void captureLocalChanges(bool force) {
  // Client: stay silent until the owner's first authoritative state has landed
  // and re-baselined us (see g_bootstrapSynced). A short grace opens the gate
  // even if that state never arrives (the download already equalled the doc),
  // so a client is never muted forever.
  if (g_mode == Mode::ClientActive && !g_bootstrapSynced) {
    if (std::chrono::steady_clock::now() - g_clientActiveSince < std::chrono::seconds(3)) return;
    g_bootstrapSynced = true;
  }

  const int cc = GetProjectStateChangeCount(nullptr);
  const auto now = std::chrono::steady_clock::now();
  // Fast path: the project change counter moved (any undoable edit). Safety
  // net: some edits reach the chunks without promptly bumping the counter
  // (FX-parameter gestures get batched undo points, some toggles defer
  // theirs), so a full re-diff also runs unconditionally on a rescan cadence:
  // the gate is a latency optimization, never a correctness dependency.
  //
  // Both cadences adapt to what a capture actually costs here (EMA over the
  // measured serialize+diff time): a small project keeps the snappy 250ms /
  // 2s floors, a huge one stretches toward the ceilings so capture stays a
  // bounded slice (~1/8th) of main-thread time instead of making REAPER's UI
  // choppy. Latency degrades gracefully; correctness never depends on it.
  // ((std::max)) defeats SWELL's max/min macros.
  const auto minGap = (std::max)(std::chrono::milliseconds(250), g_captureCost * 8);
  const auto rescanGap = (std::clamp)(g_captureCost * 20, std::chrono::milliseconds(2000),
                                      std::chrono::milliseconds(10000));
  const bool counterMoved = cc != g_lastChangeCount;
  const bool rescanDue = now - g_lastFullScan >= rescanGap;
  if (!counterMoved && !rescanDue) return;
  // Chunk serialization isn't free; cap capture rate (sync target is ~1s).
  if (!force && !rescanDue && now - g_lastCapture < minGap) return;
  g_lastCapture = now;
  g_lastFullScan = now;
  g_lastChangeCount = cc;
  const auto captureStart = std::chrono::steady_clock::now();

  internalizeExternalMedia();
  Snapshot snap = snapshotProject();

  // Opaque chunk fields (FX chains, input FX, envelope blocks, receives) carry
  // plugin state that re-serializes differently on every read even when nobody
  // touched it: a meter or analyzer plugin bakes its live display buffer into
  // its state chunk. On the bare 2s rescan (the change counter did NOT move, so
  // REAPER itself considers nothing undoable to have happened) we therefore pin
  // those fields back to the baseline, so that churn never becomes a phantom
  // edit that the owner rejects ("change overridden") or admits (re-broadcast
  // noise). A real FX edit bumps the counter and rides the fast path below,
  // where the fresh value is kept. This is the core of the spurious-override
  // fix; only scalar/structural changes (which can defer their undo point) need
  // the rescan, and they are never pinned.
  if (!counterMoved) {
    for (auto& [guid, ts] : snap.tracks) {
      auto b = g_snapshot.tracks.find(guid);
      if (b == g_snapshot.tracks.end()) continue;  // a brand-new track: keep it whole
      ts.fxchain = b->second.fxchain;
      ts.recfx = b->second.recfx;
      ts.env = b->second.env;
      ts.recv = b->second.recv;
    }
    for (auto& [guid, is] : snap.items) {
      auto b = g_snapshot.items.find(guid);
      if (b != g_snapshot.items.end()) is.chunk = b->second.chunk;
    }
  }

  // Items whose media is still being copied into the project folder stay off
  // the wire: their chunk still references the external ABSOLUTE path, which
  // no peer could ever fetch. An already-known item keeps its baseline chunk
  // (no phantom edit), a brand-new one stays invisible until the copy lands
  // and its take is repointed: never a tombstone, since it was never synced.
  for (const auto& [guid, srcPath] : g_itemsAwaitingCopy) {
    auto it = snap.items.find(guid);
    if (it == snap.items.end()) continue;
    auto b = g_snapshot.items.find(guid);
    if (b != g_snapshot.items.end())
      it->second.chunk = b->second.chunk;
    else
      snap.items.erase(it);
  }

  // Items locked by another peer are read-only locally. Restore their synced
  // state without proposing the edit. Keeping the authoritative baseline
  // also prevents the local lock tint from entering the diff.
  revertLockedItemEdits(snap);
  pinPlaceholderItems(snap);

  std::string changeset = diffSnapshots(g_snapshot, snap);
  if (!changeset.empty()) changeset = guardMediaBlanking(changeset, g_snapshot);
  // Progressive join, still loading: the project is view-only until every
  // media file is here. Snap the edit back to the baseline right away (the
  // same move a rejected lock makes) instead of proposing it, and keep the
  // baseline as it was: it IS the authoritative state.
  if (!changeset.empty() && g_mode == Mode::ClientActive && g_mediaLoading) {
    json cs = json::parse(changeset, nullptr, false);
    if (cs.is_object()) revertWhileLoading(cs, g_snapshot);
    g_lastChangeCount = GetProjectStateChangeCount(nullptr);
    activityEvent("loading_readonly", json::object(),
                  "the project is still loading. Editing unlocks once everything is here");
    return;
  }
  g_snapshot = std::move(snap);
  // Feed the adaptive throttle (EMA, 30% weight on the fresh sample) with what
  // this capture actually cost.
  const auto spent = std::chrono::duration_cast<std::chrono::milliseconds>(
      std::chrono::steady_clock::now() - captureStart);
  g_captureCost = (g_captureCost.count() == 0) ? spent : (g_captureCost * 7 + spent * 3) / 10;
  if (changeset.empty()) return;  // change count moved for something we don't sync

  // Opt-in diagnostic (launch REAPER with REAMESH_DEBUG_CAPTURE=1): name every
  // (section / id / field) a capture is about to propose and whether the change
  // counter moved. A field that streams here forever on RESCAN with no user
  // action is a frame-instability leak to strip.
  if (getenv("REAMESH_DEBUG_CAPTURE")) {
    json cs = json::parse(changeset, nullptr, false);
    std::string summary;
    if (cs.is_object())
      for (auto& sec : cs.items())
        if (sec.value().is_object())
          for (auto& ent : sec.value().items()) {
            summary += sec.key() + "/" + ent.key().substr(0, 8) + "[";
            if (ent.value().is_object())
              for (auto& f : ent.value().items()) summary += f.key() + ",";
            summary += "] ";
          }
    fprintf(stderr, "[reamesh] capture %s -> %s\n", counterMoved ? "COUNTER" : "RESCAN",
            summary.c_str());
  }

  reamesh_push_local(g_handle, changeset.c_str());
}

// ---------- apply (remote side) ----------

static void applyTrackFields(MediaTrack* tr, const json& fields) {
  if (fields.contains("name") && fields["name"].is_string()) {
    std::string name = fields["name"].get<std::string>();
    std::vector<char> buf(name.begin(), name.end());
    buf.push_back('\0');
    GetSetMediaTrackInfo_String(tr, "P_NAME", buf.data(), true);
  }
  auto setNum = [&](const char* key, const char* parm) {
    auto it = fields.find(key);
    if (it != fields.end() && it->is_number())
      SetMediaTrackInfo_Value(tr, parm, it->get<double>());
  };
  auto setBool = [&](const char* key, const char* parm) {
    auto it = fields.find(key);
    if (it != fields.end() && it->is_boolean())
      SetMediaTrackInfo_Value(tr, parm, it->get<bool>() ? 1.0 : 0.0);
  };
  setNum("vol", "D_VOL");
  setNum("pan", "D_PAN");
  // "mute" is intentionally not applied (nor captured): per-user monitoring,
  // like solo. An older peer may still send it; ignoring it here keeps the
  // local mix the user's own.
  setBool("fxen", "I_FXEN");
  setNum("width", "D_WIDTH");
  setNum("panmode", "I_PANMODE");
  setNum("dualpanl", "D_DUALPANL");
  setNum("dualpanr", "D_DUALPANR");
  setNum("panlaw", "D_PANLAW");
  setNum("panlawflags", "I_PANLAW_FLAGS");
  setBool("phase", "B_PHASE");
  setBool("solodefeat", "B_SOLO_DEFEAT");
  setNum("nchan", "I_NCHAN");
  if (fields.contains("color") && fields["color"].is_number())
    SetMediaTrackInfo_Value(tr, "I_CUSTOMCOLOR", colorFromWire(fields["color"].get<int>()));
  setNum("folderdepth", "I_FOLDERDEPTH");
  setBool("mainsend", "B_MAINSEND");
  setNum("mainsendoffs", "C_MAINSEND_OFFS");
  setNum("mainsendnch", "C_MAINSEND_NCH");
  setNum("timebase", "C_BEATATTACHMODE");
  // Flag before value: it decides whether the offset is seconds or samples.
  setNum("playoffsflag", "I_PLAY_OFFSET_FLAG");
  setNum("playoffs", "D_PLAY_OFFSET");
  // Lane layout before the per-lane fields below, so the lanes exist.
  setNum("freemode", "I_FREEMODE");
  setNum("numlanes", "I_NUMFIXEDLANES");
  setNum("lanesettings", "C_LANESETTINGS");
  if (fields.contains("lanenames") && fields["lanenames"].is_string()) {
    // Mirror capture exactly: it fills this only on a fixed-lane track, one
    // entry per lane, so a lone lane whose name was just cleared legitimately
    // arrives as "" and must still be applied. Gate on the same condition
    // (freemode/lane count as just applied above) rather than on the string
    // being non-empty, and clamp to the lane count: unclamped, an ordinary
    // track's empty field wiped the receiving user's P_LANENAME:0, and a shrunk
    // lane set left the dropped lanes' names standing.
    const int freemode = (int)GetMediaTrackInfo_Value(tr, "I_FREEMODE");
    const int lanes = (int)GetMediaTrackInfo_Value(tr, "I_NUMFIXEDLANES");
    if (freemode == 2 && lanes > 0) {
      const std::string names = fields["lanenames"].get<std::string>();
      size_t pos = 0;
      for (int lane = 0; lane < lanes && pos <= names.size(); ++lane) {
        size_t eol = names.find('\n', pos);
        if (eol == std::string::npos) eol = names.size();
        std::string n = names.substr(pos, eol - pos);
        char key[32];
        snprintf(key, sizeof(key), "P_LANENAME:%d", lane);
        std::vector<char> buf(n.begin(), n.end());
        buf.push_back('\0');
        GetSetMediaTrackInfo_String(tr, key, buf.data(), true);
        pos = eol + 1;
      }
    }
  }
  if (fields.contains("laneplays") && fields["laneplays"].is_string()) {
    const std::string plays = fields["laneplays"].get<std::string>();
    int lane = 0;
    size_t pos = 0;
    while (pos < plays.size()) {
      size_t sp = plays.find(' ', pos);
      if (sp == std::string::npos) sp = plays.size();
      char key[32];
      snprintf(key, sizeof(key), "C_LANEPLAYS:%d", lane++);
      SetMediaTrackInfo_Value(tr, key, atof(plays.substr(pos, sp - pos).c_str()));
      pos = sp + 1;
    }
  }

  // Chunk-level parts arrive as normalized blocks/lines; splice them into the
  // track's current chunk (after the scalar setters above, so their values
  // survive the chunk round-trip). Receives ("recv") are deliberately NOT
  // handled here: applyChangeset splices them in a second pass once every
  // track in the changeset exists, because their wire form references source
  // tracks by GUID.
  TrackBlocksPatch patch;
  std::string fx, recfx, env, group, lock;
  auto strField = [&](const char* key, std::string& store, const std::string*& slot) {
    auto it = fields.find(key);
    if (it != fields.end() && it->is_string()) {
      store = it->get<std::string>();
      slot = &store;
    }
  };
  strField("fxchain", fx, patch.fxchain);
  strField("recfx", recfx, patch.recfx);
  strField("env", env, patch.env);
  strField("group", group, patch.group);
  strField("lock", lock, patch.lock);
  if (patch.fxchain || patch.recfx || patch.env || patch.group || patch.lock) {
    std::string cur = objectStateChunk(tr);
    if (!cur.empty()) {
      std::string next = replaceTrackBlocks(cur, patch);
      if (next != cur) setObjectStateChunk(tr, next);
    }
  }
}

// Reorder current tracks toward the index hints in the changeset: current
// order, overridden by the hinted indices, stable-sorted. O(n^2) track moves
// in the worst case, fine at project scale.
static void applyReorder(const std::map<std::string, double>& desiredIndex) {
  struct Entry {
    std::string guid;
    double index;
  };
  std::vector<Entry> order;
  const int n = CountTracks(nullptr);
  for (int i = 0; i < n; ++i) {
    std::string guid = trackGuid(GetTrack(nullptr, i));
    auto it = desiredIndex.find(guid);
    order.push_back({guid, it != desiredIndex.end() ? it->second : (double)i});
  }
  std::stable_sort(order.begin(), order.end(), [](const Entry& a, const Entry& b) {
    if (a.index != b.index) return a.index < b.index;
    return a.guid < b.guid;  // deterministic tie-break on all peers
  });
  // ReorderSelectedTracks works on the selection, which is per-user state
  // (and what the presence beacon reports as this user's focus): remember it
  // and put it back once the tracks are in place.
  std::vector<std::string> selected;
  for (int i = 0, n = CountSelectedTracks(nullptr); i < n; ++i)
    selected.push_back(trackGuid(GetSelectedTrack(nullptr, i)));
  bool moved = false;
  for (int i = 0; i < (int)order.size(); ++i) {
    if (trackGuid(GetTrack(nullptr, i)) == order[i].guid) continue;
    MediaTrack* want = findTrackByGuid(order[i].guid);
    if (!want) continue;
    SetOnlyTrackSelected(want);
    ReorderSelectedTracks(i, 0);
    moved = true;
  }
  if (!moved) return;
  for (int i = 0, n = CountTracks(nullptr); i < n; ++i) SetTrackSelected(GetTrack(nullptr, i), false);
  for (const std::string& guid : selected)
    if (MediaTrack* tr = findTrackByGuid(guid)) SetTrackSelected(tr, true);
}

// ---- peak building for media that arrived mid-session ----
//
// REAPER doesn't reliably redraw an item whose source file appeared after the
// item was drawn: the background peak build ("build missing peaks") finished,
// but the waveform only showed up once the item was clicked. So peaks for
// freshly fetched media are built here through PCM_Source_BuildPeaks, in small
// time slices on the main thread, and the arrange redraws when each finishes.
// Jobs are keyed by item GUID and re-resolve their sources every tick (a track
// chunk set rebuilds the items and their sources); a source is only ever
// advanced right after being resolved from the live item.
struct PeakJob {
  std::set<PCM_source*> begun;  // sources whose build we started
  std::set<PCM_source*> done;   // sources with finished (or already valid) peaks
  std::chrono::steady_clock::time_point since;
};
static std::map<std::string, PeakJob> g_peakJobs;
static constexpr std::chrono::seconds PEAK_JOB_MAX_AGE{180};
static constexpr std::chrono::milliseconds PEAK_TICK_BUDGET{8};

static void queuePeakBuild(const std::string& guid) {
  // A new chunk may have brought new sources: (re)start the job.
  g_peakJobs[guid] = PeakJob{{}, {}, std::chrono::steady_clock::now()};
}

static PCM_source* rootSource(PCM_source* src) {
  // Section/reverse wrappers build through the file source they wrap.
  for (int guard = 0; src && guard < 8; ++guard) {
    PCM_source* parent = GetMediaSourceParent(src);
    if (!parent) break;
    src = parent;
  }
  return src;
}

static void processPeakJobs() {
  if (g_peakJobs.empty()) return;
  const auto now = std::chrono::steady_clock::now();
  const auto deadline = now + PEAK_TICK_BUDGET;
  bool redraw = false;
  for (auto job = g_peakJobs.begin(); job != g_peakJobs.end();) {
    MediaItem* item = findItemByGuid(job->first);
    if (!item || now - job->second.since > PEAK_JOB_MAX_AGE) {
      job = g_peakJobs.erase(job);
      continue;
    }
    bool pending = false;
    for (int t = 0, n = CountTakes(item); t < n; ++t) {
      MediaItem_Take* take = GetMediaItemTake(item, t);
      PCM_source* src = take ? rootSource(GetMediaItemTake_Source(take)) : nullptr;
      if (!src || job->second.done.count(src)) continue;
      if (!job->second.begun.count(src)) {
        if (PCM_Source_BuildPeaks(src, 0) == 0) {  // peaks already valid
          job->second.done.insert(src);
          redraw = true;
          continue;
        }
        job->second.begun.insert(src);
      }
      bool finished = false;
      while (std::chrono::steady_clock::now() < deadline) {
        if (PCM_Source_BuildPeaks(src, 1) == 0) {
          PCM_Source_BuildPeaks(src, 2);
          finished = true;
          break;
        }
      }
      if (finished) {
        job->second.done.insert(src);
        redraw = true;
      } else {
        pending = true;
      }
    }
    if (!pending) {
      UpdateItemInProject(item);
      job = g_peakJobs.erase(job);
    } else {
      ++job;
    }
    if (std::chrono::steady_clock::now() >= deadline) break;
  }
  if (redraw) UpdateArrange();
}

// Media files this item needs that are missing from the project folder.
static std::vector<std::string> missingMediaIn(const json& fields) {
  std::vector<std::string> missing;
  if (!fields.contains("chunk") || !fields["chunk"].is_string()) return missing;
  const fs::path projDir = projectDir();
  for (std::string p : chunkFilePaths(fields["chunk"].get<std::string>())) {
    if (!isRelativePath(p)) continue;  // absolute: nothing we can fetch
    std::replace(p.begin(), p.end(), '\\', '/');
    std::error_code ec;
    if (!fs::exists(projDir / fsPath(p), ec)) missing.push_back(p);
  }
  return missing;
}

// Apply one item's fields (creation, move across tracks, content update,
// deletion). Caller has verified the referenced media exists.
static void applyItem(const std::string& guid, const json& fields, bool freshMedia = false) {
  // Any remote change to this item restores its real (synced) color, so drop a
  // tint record we hold for it: reconcileItemTints will re-stash the FRESH real
  // color and re-tint if the item is still locked. This is what keeps a tinted
  // item's restore color current when its real color changes mid-lock. The
  // grey has to actually leave the item here, though: a change without a
  // chunk (a move to another track) or a chunk without a COLOR line (an item in
  // the default colour) would otherwise keep it, the next reconcile would stash
  // that grey as the "real" colour, and the item would stay grey for good.
  int tintedReal = -1;
  if (auto rec = g_itemTints.find(guid); rec != g_itemTints.end()) {
    if (!rec->second.expired) tintedReal = rec->second.real;
    g_itemTints.erase(rec);
  }
  MediaItem* item = findItemByGuid(guid);
  if (!jsonBool(fields, "present", true)) {
    if (item) DeleteTrackMediaItem(GetMediaItem_Track(item), item);
    g_placeholders.erase(guid);  // the owner removed it: nothing left to stand in for
    g_peakJobs.erase(guid);
    return;
  }
  MediaTrack* tr = nullptr;
  if (fields.contains("track") && fields["track"].is_string())
    tr = findTrackByGuid(fields["track"].get<std::string>());
  bool created = false;
  if (!item) {
    if (!tr) return;  // no destination track (yet); the full-sync will retry
    item = AddMediaItemToTrack(tr);
    if (!item) return;
    created = true;
  } else if (tr && GetMediaItem_Track(item) != tr) {
    MoveMediaItemToTrack(item, tr);
  }
  bool resolvedPlaceholder = false;
  bool fileBacked = false;
  if (fields.contains("chunk") && fields["chunk"].is_string()) {
    // The chunk carries the originator's item GUID, so identity is stamped
    // onto newly created items as a side effect.
    const std::string& chunk = fields["chunk"].get_ref<const std::string&>();
    const std::vector<std::string>& dirs = mediaPathDirs();
    setObjectStateChunk(item, absolutizeChunkFiles(chunk, dirs.empty() ? "" : dirs.front()));
    if (!itemChunkHasColor(chunk)) SetMediaItemInfo_Value(item, "I_CUSTOMCOLOR", 0.0);
    fileBacked = chunkHasFileSource(chunk);
    // A file-backed chunk landing on a progressive-join placeholder is its
    // media arriving: the stand-in is resolved. (A chunk without a FILE line
    // may be the placeholder itself being re-applied by a local snap-back.)
    if (fileBacked && g_placeholders.erase(guid)) resolvedPlaceholder = true;
  } else if (tintedReal >= 0) {
    SetMediaItemInfo_Value(item, "I_CUSTOMCOLOR", static_cast<double>(tintedReal));
  }
  UpdateItemInProject(item);
  // Media that only just reached this machine has no peaks yet: build them
  // and redraw once done, or the waveform only appears after a click.
  if (fileBacked && (freshMedia || created || resolvedPlaceholder)) queuePeakBuild(guid);
}

// Names the peer currently holding the editing lock on `(section, id)`: the
// best available answer to "who made this change" (authoritative updates carry
// no author; the active editor holds the lock while their edit lands).
//
// The lock table and the roster are the same for every entity in a changeset,
// and each is an FFI call plus a full JSON parse, so one instance of this is
// kept per applyChangeset and loaded lazily: a bootstrap apply touches
// hundreds of entities but almost never needs a holder name for any of them.
struct LockHolders {
  json locks;
  json peers;
  bool loaded = false;

  std::string name(const char* section, const std::string& id) {
    if (!g_handle) return "";
    if (!loaded) {
      loaded = true;
      locks = handleJson(reamesh_locked_entities, json::array());
      peers = handleJson(reamesh_peers, json::array());
    }
    if (!locks.is_array()) return "";
    for (const auto& e : locks)
      if (e.is_object() && jsonStr(e, "section") == section && jsonStr(e, "id") == id)
        return peerNameById(peers, jsonStr(e, "holder"));
    return "";
  }
};

// Report plugins introduced by remote changes, for example "Dan added Pro-Q 3
// to Drums". This keeps later plugin loads as visible as the initial join. One
// event per plugin, collapsed to a single summary when a whole chain lands at
// once (pasting a track with 15 plugins is one notice, not fifteen).
//
// `byFn` resolves the lock holder's name and is only called once there is
// something to narrate: it is by far the most expensive thing here, and the
// common case (a changeset that adds no plugins) must not pay for it.
static void notePluginAdds(const std::string& trackName,
                           const std::function<std::string()>& byFn,
                           const std::vector<std::string>& oldPlugs,
                           const std::vector<std::string>& newPlugs) {
  std::set<std::string> oldSet(oldPlugs.begin(), oldPlugs.end());
  std::vector<std::string> added;
  for (const std::string& p : newPlugs)
    if (!oldSet.count(p)) added.push_back(p);
  if (added.empty()) return;
  const std::string by = byFn();
  if (added.size() > 5) {
    json params = {{"count", added.size()}};
    if (!trackName.empty()) params["track"] = trackName;
    if (!by.empty()) params["by"] = by;
    activityEvent("plugins_added", params,
                  std::to_string(added.size()) + " plugins were added" +
                      (trackName.empty() ? "" : " to " + trackName));
    return;
  }
  for (const std::string& p : added) {
    json params = {{"plugin", p}};
    if (!trackName.empty()) params["track"] = trackName;
    if (!by.empty()) params["by"] = by;
    std::string text = by.empty() ? p + " was added" : by + " added " + p;
    if (!trackName.empty()) text += " to " + trackName;
    activityEvent("plugin_added", params, text);
  }
}

// Splice a track's receives into its live chunk. The wire form carries the
// source track as a GUID (indices are per-peer and move on every reorder), so
// each unit is resolved to a local track index here; a unit whose source track
// doesn't exist locally is dropped from the splice, and `false` comes back so
// the caller can park the field for a retry. Dropping it outright would lose
// the routing for good: the owner's doc still holds the identical `recv`
// string, so no later diff would ever re-send it.
// `wrote` (optional) reports whether the track's chunk actually changed, so a
// retry that resolved nothing new costs no re-baseline.
static bool applyRecvField(const std::string& guid, const std::string& wire,
                           bool* wrote = nullptr) {
  MediaTrack* tr = findTrackByGuid(guid);
  if (!tr) return false;
  bool complete = true;
  std::string local = transformRecvUnits(wire, [&complete](const std::string& tok) -> std::string {
    MediaTrack* src = findTrackByGuid(tok);
    if (!src) {
      complete = false;
      return "";
    }
    const int n = (int)GetMediaTrackInfo_Value(src, "IP_TRACKNUMBER");
    if (n <= 0) {
      complete = false;
      return "";
    }
    return std::to_string(n - 1);
  });
  std::string cur = objectStateChunk(tr);
  if (cur.empty()) return false;
  // Already spliced (a processDeferredRecv retry, or the same value sent again)?
  // Decide in NORMALIZED form: `local` came off the wire LF-only with the send
  // envelopes' ARM/VIS lines stripped, while REAPER re-serializes the track with
  // the platform's line ending and puts those lines back. A raw byte compare of
  // the re-spliced chunk against the live one therefore never matches (always
  // on Windows, and on every platform once a receive carries a send envelope),
  // and each retry would rebuild the whole track: every tick, for up to 90s.
  if (extractTrackBlocks(normalizeChunk(cur)).recv == local) return complete;
  TrackBlocksPatch patch;
  patch.recv = &local;
  std::string next = replaceTrackBlocks(cur, patch);
  if (next != cur) {
    setObjectStateChunk(tr, next);
    if (wrote) *wrote = true;
  }
  return complete;
}

// No-FX mode: drop fx-chain fields from track applies and take-FX blocks from
// item chunks, so a view-only client never instantiates third-party code.
static json filterChangesetNoFx(json cs) {
  if (cs.contains("tracks") && cs["tracks"].is_object()) {
    for (auto& [guid, fields] : cs["tracks"].items()) {
      (void)guid;
      if (fields.is_object()) {
        fields.erase("fxchain");
        fields.erase("recfx");
      }
    }
  }
  if (cs.contains("items") && cs["items"].is_object()) {
    for (auto& [guid, fields] : cs["items"].items()) {
      (void)guid;
      if (fields.is_object() && fields.contains("chunk") && fields["chunk"].is_string())
        fields["chunk"] = stripFxBlocks(fields["chunk"].get<std::string>(), /*takeFxOnly=*/true);
    }
  }
  return cs;
}

static void applyChangeset(const json& changeset) {
  // One lazily-loaded lock table + roster for the whole changeset (see
  // LockHolders); resolving a holder per entity was an FFI call and a full
  // JSON parse each time.
  LockHolders holders;
  if (changeset.contains("tracks") && changeset["tracks"].is_object()) {
    const json& tracks = changeset["tracks"];

    // Deletions first so insertion indices below land in a settled list.
    for (auto it = tracks.begin(); it != tracks.end(); ++it) {
      const json& fields = it.value();
      if (fields.is_object() && !jsonBool(fields, "present", true)) {
        if (MediaTrack* tr = findTrackByGuid(it.key())) {
          std::string nm = trackNameByGuid(it.key());  // name before it's gone
          DeleteTrack(tr);
          if (nm.empty())
            activityEvent("peer_track_removed", json::object(), "a track was removed");
          else
            activityEvent("peer_track_removed", {{"name", nm}}, "removed track " + nm);
        }
      }
    }

    std::map<std::string, double> desiredIndex;
    std::vector<std::pair<std::string, std::string>> pendingRecv;  // guid -> wire recv
    for (auto it = tracks.begin(); it != tracks.end(); ++it) {
      const std::string& guid = it.key();
      const json& fields = it.value();
      if (!fields.is_object() || !jsonBool(fields, "present", true)) continue;

      MediaTrack* tr = findTrackByGuid(guid);
      bool created = false;
      if (!tr) {
        int idx = (int)jsonNum(fields, "index", (double)CountTracks(nullptr));
        idx = std::clamp(idx, 0, CountTracks(nullptr));
        InsertTrackAtIndex(idx, false);
        tr = GetTrack(nullptr, idx);
        if (!tr) continue;
        // Stamp the originator's GUID onto the new track so identity is
        // shared across all peers (indices are never used as identity).
        std::vector<char> gbuf(guid.begin(), guid.end());
        gbuf.push_back('\0');
        GetSetMediaTrackInfo_String(tr, "GUID", gbuf.data(), true);
        created = true;
      }
      // Narrate plugins this change introduces BEFORE the splice (the diff
      // needs the pre-apply baseline). The baseline is the last synced state
      // of this track; a brand-new track counts everything it carries.
      if (fields.contains("fxchain") || fields.contains("recfx")) {
        auto base = g_snapshot.tracks.find(guid);
        std::string oldTxt, newTxt;
        if (base != g_snapshot.tracks.end()) oldTxt = base->second.fxchain + base->second.recfx;
        newTxt += (fields.contains("fxchain") && fields["fxchain"].is_string())
                      ? fields["fxchain"].get<std::string>()
                      : (base != g_snapshot.tracks.end() ? base->second.fxchain : "");
        newTxt += (fields.contains("recfx") && fields["recfx"].is_string())
                      ? fields["recfx"].get<std::string>()
                      : (base != g_snapshot.tracks.end() ? base->second.recfx : "");
        std::string nm = jsonStr(fields, "name");
        if (nm.empty()) nm = trackNameByGuid(guid);
        notePluginAdds(nm, [&] { return holders.name("tracks", guid); },
                       collectPlugins(oldTxt), collectPlugins(newTxt));
      }
      applyTrackFields(tr, fields);
      if (created) {
        std::string nm = jsonStr(fields, "name");
        if (nm.empty())
          activityEvent("peer_track_added", json::object(), "a track was added");
        else
          activityEvent("peer_track_added", {{"name", nm}}, "added track " + nm);
      }
      if (fields.contains("recv") && fields["recv"].is_string())
        pendingRecv.emplace_back(guid, fields["recv"].get<std::string>());
      if (fields.contains("index") && fields["index"].is_number())
        desiredIndex[guid] = fields["index"].get<double>();
    }
    if (!desiredIndex.empty()) applyReorder(desiredIndex);

    // Receives last, after every track in the changeset exists and the order
    // is settled: the wire form carries source-track GUIDs, resolved to local
    // indices here. A receive whose source track isn't here yet is parked and
    // retried by processDeferredRecv: nothing would ever re-send it otherwise.
    for (const auto& [guid, wire] : pendingRecv) {
      if (applyRecvField(guid, wire))
        g_deferredRecv.erase(guid);
      else
        g_deferredRecv[guid] = {wire, std::chrono::steady_clock::now()};
    }
  }

  if (changeset.contains("items") && changeset["items"].is_object()) {
    const json& items = changeset["items"];
    for (auto it = items.begin(); it != items.end(); ++it) {
      const std::string& guid = it.key();
      const json& fields = it.value();
      if (!fields.is_object()) continue;

      // Take-FX plugins arriving inside an item chunk get the same narration
      // as track-chain arrivals.
      if (fields.contains("chunk") && fields["chunk"].is_string() &&
          jsonBool(fields, "present", true)) {
        auto base = g_snapshot.items.find(guid);
        const std::string oldTxt =
            base != g_snapshot.items.end() ? base->second.chunk : std::string();
        std::string trGuid = jsonStr(fields, "track");
        if (trGuid.empty() && base != g_snapshot.items.end()) trGuid = base->second.track;
        notePluginAdds(trackNameByGuid(trGuid), [&] { return holders.name("items", guid); },
                       collectPlugins(oldTxt),
                       collectPlugins(fields["chunk"].get<std::string>()));
      }

      // A tombstone or retarget to available media supersedes a parked copy;
      // otherwise old fields could reappear when the media arrives.
      std::erase_if(g_deferredItems, [&](const DeferredItem& d) { return d.guid == guid; });
      std::vector<std::string> missing = missingMediaIn(fields);
      if (!missing.empty()) {
        // Media hasn't arrived yet: fetch it from the peers and apply the
        // item once it exists. Latest fields win for the same item.
        for (const std::string& rel : missing) reamesh_fetch_file(g_handle, rel.c_str());
        g_deferredItems.push_back(
            {guid, fields, std::move(missing), std::chrono::steady_clock::now()});
        // A progressive-join placeholder waiting for its media is the expected
        // case (hundreds at once on a big project): the overlay's progress
        // strip narrates that; only a genuinely new item gets a log line.
        if (!g_placeholders.count(guid))
          logEvent("waiting_media", json::object(), "waiting for media of an incoming item...");
        continue;
      }
      applyItem(guid, fields);
    }
  }

  // Project-wide state: rebuild the tempo map and/or marker list when they
  // change (each is whole-field, so a single edit re-emits the whole list).
  if (changeset.contains("project") && changeset["project"].is_object()) {
    auto it = changeset["project"].find("project");
    if (it != changeset["project"].end() && it->is_object()) {
      const json& f = *it;
      if (f.contains("tempo") && f["tempo"].is_string()) {
        applyTempo(f["tempo"].get<std::string>());
        activityEvent("peer_tempo_changed", json::object(), "the tempo map changed");
      }
      if (f.contains("markers") && f["markers"].is_string()) {
        applyMarkers(f["markers"].get<std::string>());
        activityEvent("peer_markers_changed", json::object(), "markers/regions changed");
      }
      if (f.contains("notes") && f["notes"].is_string()) {
        applyProjectNotes(f["notes"].get<std::string>());
        activityEvent("peer_notes_changed", json::object(), "project notes changed");
      }
      if (f.contains("title") && f["title"].is_string()) {
        applyProjectString("PROJECT_TITLE", f["title"].get<std::string>());
        activityEvent("peer_title_changed", json::object(), "project info changed");
      }
      if (f.contains("author") && f["author"].is_string()) {
        applyProjectString("PROJECT_AUTHOR", f["author"].get<std::string>());
        activityEvent("peer_title_changed", json::object(), "project info changed");
      }
      if (f.contains("groupnames") && f["groupnames"].is_string())
        applyGroupNames(f["groupnames"].get<std::string>());
      if (f.contains("timebase") && f["timebase"].is_string())
        applyProjectTimebase(f["timebase"].get<std::string>());
    }
  }
}

// Capture before re-baselining. Every site that writes to the project under
// g_applyingRemote and then re-baselines g_snapshot (remote apply, deferred
// item/receive flush, lock-tint paint) would otherwise fold in whatever local
// edit the capture throttle skipped since the last capture, and that edit
// would never be proposed, because the next diff compares it against itself.
// So the pending local delta is captured first, throttle or not. Gated on the
// change counter, so it costs nothing while the user isn't touching anything.
static void flushLocalEdits() {
  if (g_mode != Mode::Hosting && g_mode != Mode::ClientActive) return;
  if (g_noFx || g_applyingRemote) return;
  if (GetProjectStateChangeCount(nullptr) == g_lastChangeCount) return;
  captureLocalChanges(/*force=*/true);
}

static void applyRemoteChanges() {
  char* raw = reamesh_drain_remote(g_handle);
  std::string payload = raw ? raw : "[]";
  reamesh_free_string(raw);
  if (payload.empty() || payload == "[]") return;

  json changesets = json::parse(payload, nullptr, /*allow_exceptions=*/false);
  if (!changesets.is_array() || changesets.empty()) return;
  flushLocalEdits();

  // Echo suppression: the setters below bump GetProjectStateChangeCount; with
  // the flag set and the re-baseline at the end, the capture loop never sees
  // (and never re-broadcasts) the delta we are about to create.
  g_applyingRemote = true;
  // No undo point for remote edits: Ctrl+Z must only ever target the user's
  // own actions, never silently roll back a peer's work. (REAPER undo is
  // snapshot-based, so undoing your own last action can still take remote
  // edits that landed after it along for the ride: that revert is captured
  // as a normal local change next tick and synced back out; there is no
  // re-application loop because we re-baseline below.)
  PreventUIRefresh(1);
  for (const auto& cs : changesets) {
    // A changeset is peer-authored JSON: a shape the apply code doesn't expect
    // drops that changeset, never unwinds through REAPER's timer.
    try {
      applyChangeset(g_noFx ? filterChangesetNoFx(cs) : cs);
    } catch (const std::exception& e) {
      logEvent("bad_changeset", {{"error", e.what()}},
               std::string("dropped a malformed incoming edit: ") + e.what());
    }
  }
  PreventUIRefresh(-1);
  TrackList_AdjustWindows(false);
  UpdateArrange();

  g_snapshot = snapshotProject();
  g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  // The owner's authoritative state has landed and is now our baseline; a
  // client may start proposing its own edits from here (see captureLocalChanges).
  if (g_mode == Mode::ClientActive) g_bootstrapSynced = true;
  g_applyingRemote = false;
}

// Put every entity a local edit touched (per `changeset`, the capture diff)
// back to the baseline `base`: tracks and items the baseline knows are
// re-applied whole, ones it doesn't (created locally) are removed, and changed
// project fields are restored. Echo-suppressed, no undo point, like a remote
// apply. Used while a progressive join is still loading (view-only phase).
static void revertWhileLoading(const json& changeset, const Snapshot& base) {
  json revert = json::object();
  if (changeset.contains("tracks") && changeset["tracks"].is_object()) {
    json tracks = json::object();
    for (auto it = changeset["tracks"].begin(); it != changeset["tracks"].end(); ++it) {
      auto b = base.tracks.find(it.key());
      tracks[it.key()] = b == base.tracks.end() ? json{{"present", false}} : trackToJson(b->second);
    }
    revert["tracks"] = std::move(tracks);
  }
  if (changeset.contains("items") && changeset["items"].is_object()) {
    json items = json::object();
    for (auto it = changeset["items"].begin(); it != changeset["items"].end(); ++it) {
      auto b = base.items.find(it.key());
      items[it.key()] = b == base.items.end()
                            ? json{{"present", false}}
                            : json{{"present", true}, {"track", b->second.track}, {"chunk", b->second.chunk}};
    }
    revert["items"] = std::move(items);
  }
  if (changeset.contains("project") && changeset["project"].is_object() &&
      changeset["project"].contains("project") && changeset["project"]["project"].is_object()) {
    json proj = json::object();
    const json& changed = changeset["project"]["project"];
    if (changed.contains("tempo")) proj["tempo"] = base.project.tempo;
    if (changed.contains("markers")) proj["markers"] = base.project.markers;
    if (changed.contains("notes")) proj["notes"] = base.project.notes;
    if (changed.contains("title")) proj["title"] = base.project.title;
    if (changed.contains("author")) proj["author"] = base.project.author;
    if (changed.contains("groupnames")) proj["groupnames"] = base.project.groupnames;
    if (changed.contains("timebase")) proj["timebase"] = base.project.timebase;
    if (!proj.empty()) revert["project"] = json{{"project", std::move(proj)}};
  }
  if (revert.empty()) return;
  g_applyingRemote = true;
  PreventUIRefresh(1);
  try {
    applyChangeset(revert);
  } catch (const std::exception& e) {
    logEvent("bad_changeset", {{"error", e.what()}},
             std::string("couldn't undo an edit made while loading: ") + e.what());
  }
  PreventUIRefresh(-1);
  TrackList_AdjustWindows(false);
  UpdateArrange();
  g_applyingRemote = false;
}

// Retry items that were waiting for media files to arrive from the peers.
static void processDeferredItems() {
  if (g_deferredItems.empty()) return;
  const fs::path projDir = projectDir();
  const auto now = std::chrono::steady_clock::now();
  std::vector<DeferredItem> ready;

  std::erase_if(g_deferredItems, [&](DeferredItem& d) {
    std::error_code ec;
    std::erase_if(d.missing,
                  [&](const std::string& rel) { return fs::exists(projDir / fsPath(rel), ec); });
    if (d.missing.empty()) {
      ready.push_back(std::move(d));
      return true;
    }
    // While the progressive join's background download is still streaming,
    // the file is simply queued: the give-up clock only starts once it ends.
    if (g_downloadBackground) d.since = now;
    if (now - d.since > std::chrono::seconds(90)) {
      logEvent("media_timeout", {{"file", d.missing.front()}},
               "gave up waiting for media: " + d.missing.front());
      return true;
    }
    // Keep nudging the fetch; it is idempotent while the file is missing.
    for (const std::string& rel : d.missing) reamesh_fetch_file(g_handle, rel.c_str());
    return false;
  });

  if (ready.empty()) return;
  flushLocalEdits();
  g_applyingRemote = true;
  PreventUIRefresh(1);
  for (const DeferredItem& d : ready) {
    try {
      applyItem(d.guid, d.fields, /*freshMedia=*/true);
    } catch (const std::exception& e) {
      logEvent("bad_changeset", {{"error", e.what()}},
               std::string("dropped a malformed incoming edit: ") + e.what());
    }
  }
  PreventUIRefresh(-1);
  UpdateArrange();
  g_snapshot = snapshotProject();
  g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  g_applyingRemote = false;
  // The media behind these items was just fetched, so it has no local peaks
  // yet: applyItem queued them for processPeakJobs, which builds the peaks and
  // redraws when they're done (so the items don't sit there as flat blocks).
  // While a progressive join is still loading, items land in a steady trickle
  // and the progress strip tells the story; a per-batch line would flood the feed.
  if (!g_mediaLoading)
    logEvent("media_applied", {{"count", ready.size()}},
             "applied " + std::to_string(ready.size()) + " item(s) after media arrived");
}

// Retry receives parked because their source track hadn't arrived yet. Same
// shape as processDeferredItems: echo-suppressed, no undo point, re-baselined,
// and given up on after 90s so a source track that is never coming can't leave
// an entry behind forever.
static std::chrono::steady_clock::time_point g_lastRecvRetry{};

static void processDeferredRecv() {
  if (g_deferredRecv.empty()) return;
  const auto now = std::chrono::steady_clock::now();
  // Once a second is plenty: every retry serializes the parked track's chunk
  // (and resolves each unit's source by a GUID scan), and a source track can
  // only ever show up through a remote apply, never between two timer ticks.
  if (now - g_lastRecvRetry < std::chrono::seconds(1)) return;
  flushLocalEdits();
  g_lastRecvRetry = now;
  bool changed = false;
  g_applyingRemote = true;
  PreventUIRefresh(1);
  for (auto it = g_deferredRecv.begin(); it != g_deferredRecv.end();) {
    const bool complete = applyRecvField(it->first, it->second.wire, &changed);
    if (complete || now - it->second.since > std::chrono::seconds(90))
      it = g_deferredRecv.erase(it);
    else
      ++it;
  }
  PreventUIRefresh(-1);
  // Only a retry that actually spliced something needs the re-baseline; the
  // common case is a parked entry still waiting, which writes nothing.
  if (changed) {
    TrackList_AdjustWindows(false);
    UpdateArrange();
    g_snapshot = snapshotProject();
    g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  }
  g_applyingRemote = false;
}

// ---------- session lifecycle ----------

static std::string currentProjectPath() {
  char fn[2048] = "";
  EnumProjects(-1, fn, sizeof(fn));
  return fn;
}

static std::string fileStem(const std::string& path) {
  std::string p = path;
  size_t slash = p.find_last_of("/\\");
  if (slash != std::string::npos) p = p.substr(slash + 1);
  size_t dot = p.find_last_of('.');
  if (dot != std::string::npos && dot > 0) p = p.substr(0, dot);
  return p;
}

// Defined in the control-server section below; needed by the reconnect logic.
static std::string jsonStr(const json& j, const char* key);
static bool jsonBool(const json& j, const char* key, bool dflt);
static double jsonNum(const json& j, const char* key, double dflt);
static json handleJson(char* (*fn)(ReameshHandle*), const json& fallback);

// ---------- item-tint lock signal (local view transform only) ----------
//
// When another peer holds the editing lock on a media item, that item is painted
// dark grey locally ("the WAV is grey → someone's working on it"). This is a
// pure VIEW transform: the color is written into REAPER but the snapshot is
// re-baselined right after, so it never enters the capture diff and is never
// captured, proposed, synced, or written into anyone's .rpp. Our OWN held items
// are never tinted (reamesh_locked_entities filters our own holds out).
//
// The one place the tint could leak to disk is a project SAVE, which serializes
// the live (grey) item color. Two layers keep it out:
//   • Before the save, where we can hook it: dispatchCommand intercepts the
//     save actions (Ctrl+S, Save As, Save new version) and the actions that
//     end in a "save changes?" prompt (Quit, Close project, Close all, New /
//     Open project), and calls restoreAllItemTints() first, so REAPER writes
//     the REAL colors. While the prompt is up the timer doesn't tick, so
//     nothing re-greys underneath it; the next tick re-applies the tints (or
//     a cancelled quit simply carries on). leaveSession restores them too, and
//     a crash writes nothing.
//   • Everywhere else: the AUTO-SAVE, the Windows close button and whatever
//     else saves without an action we can see. There the save writes the grey,
//     but never alone: tintLedgerSave adds one `REAMESH_TINT <guid> <real>
//     <tint>` line per tinted item to every project save, so the file always
//     carries the way back. On the next load (outside a session) the ledger is
//     read back and every item still in exactly that grey gets its real colour
//     again (healPersistedTints). The ledger only exists in a file that holds
//     grey; a clean save writes none. The "atexit" hook stays registered, but
//     REAPER most likely calls it only after the quit prompt was answered, so
//     it is not what keeps a quit-time save clean.

// The dark grey a locked item is tinted with. Computed LOCALLY: never
// colorToWire, since this color never rides the wire; the high 0x1000000 bit is
// REAPER's "custom color is set" flag.
static int lockTintNative() { return 0x1000000 | ColorToNative(64, 64, 64); }

// Which action ids we intercept (in dispatchCommand) to drop the tints before
// REAPER serializes the project, so grey never reaches the .rpp: the saves
// themselves and the actions that can end in a "save changes?" prompt.
static bool isSaveCommand(int command) {
  switch (command) {
    case 40026:  // File: Save project
    case 40022:  // File: Save project as…
    case 41895:  // File: Save new version of project
    case 40004:  // File: Quit REAPER
    case 40860:  // Close current project tab
    case 40886:  // File: Close all projects
    case 40023:  // File: New project
    case 40025:  // File: Open project
      return true;
    default:
      return false;
  }
}

// Item GUIDs whose editing lock is currently held by a peer OTHER than us, from
// the live lock table Rust maintains. These are the items to show as grey.
static std::set<std::string> lockedItemsByOthers() {
  std::set<std::string> out;
  if (!g_handle) return out;
  char* raw = reamesh_locked_entities(g_handle);
  std::string payload = raw ? raw : "[]";
  reamesh_free_string(raw);
  json arr = json::parse(payload, nullptr, false);
  if (!arr.is_array()) return out;
  for (const auto& e : arr) {
    if (!e.is_object() || jsonStr(e, "section") != "items") continue;
    std::string id = jsonStr(e, "id");
    if (!id.empty()) out.insert(std::move(id));
  }
  return out;
}

// Restore every tinted item to its real color and forget the tints. Used before
// a deliberate SAVE (so grey never reaches the .rpp) and whenever the session
// ends. Echo-suppressed and re-baselined like a remote apply, so the restore is
// never seen as a local edit and makes no undo point. After this the next
// reconcile tick re-applies whatever is still locked.
static void restoreAllItemTints() {
  if (g_itemTints.empty()) return;
  const bool inSession = g_mode == Mode::Hosting || g_mode == Mode::ClientActive;
  if (inSession) flushLocalEdits();
  g_applyingRemote = true;
  for (const auto& [guid, rec] : g_itemTints)
    if (MediaItem* it = findItemByGuid(guid))
      SetMediaItemInfo_Value(it, "I_CUSTOMCOLOR", static_cast<double>(rec.real));
  g_itemTints.clear();
  UpdateArrange();
  if (inSession) {
    g_snapshot = snapshotProject();
    g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  }
  g_applyingRemote = false;
}

// ---- persisted-tint ledger ----
//
// Every project save (any path, auto-save included) calls tintLedgerSave, which
// records the tinted items' real colours next to the grey they were saved
// with. Loading such a file collects the lines (tintLedgerLine) and the next
// timer tick puts the real colours back (healPersistedTints). Undo states are
// left alone: they never reach a file.
struct TintLedgerEntry {
  int real;
  int tint;
};
static std::map<std::string, TintLedgerEntry> g_tintHeal;
static ReaProject* g_tintHealProject = nullptr;

static void tintLedgerSave(ProjectStateContext* ctx, bool isUndo, project_config_extension_t*) {
  if (isUndo || !ctx || g_itemTints.empty()) return;
  // The tints live in the active project; another tab saving is not ours.
  if (GetCurrentProjectInLoadSave() != EnumProjects(-1, nullptr, 0)) return;
  const int tint = lockTintNative();
  for (const auto& [guid, rec] : g_itemTints)
    if (!rec.expired)  // an expired tint already shows the real colour
      ctx->AddLine("REAMESH_TINT %s %d %d", guid.c_str(), rec.real, tint);
}

static bool tintLedgerLine(const char* line, ProjectStateContext*, bool isUndo,
                           project_config_extension_t*) {
  std::string_view t = ltrim(line ? std::string_view(line) : std::string_view());
  if (t.rfind("REAMESH_TINT ", 0) != 0) return false;
  if (isUndo) return true;
  char guid[96] = {};
  int real = 0, tint = 0;
  const std::string rest(t.substr(13));
  if (std::sscanf(rest.c_str(), "%95s %d %d", guid, &real, &tint) == 3) {
    g_tintHeal[guid] = {real, tint};
    g_tintHealProject = GetCurrentProjectInLoadSave();
  }
  return true;
}

static void tintLedgerBeginLoad(bool isUndo, project_config_extension_t*) {
  if (isUndo) return;
  g_tintHeal.clear();
  g_tintHealProject = nullptr;
}

static project_config_extension_t g_tintLedgerReg = {tintLedgerLine, tintLedgerSave,
                                                     tintLedgerBeginLoad, nullptr};

// Put back the real colours a loaded file's ledger names, on the tick after the
// load (the items exist by then). Only outside a session: a joiner's copy gets
// its colours from the owner's doc, and a session's own capture must never see
// this as an edit. An item someone has recoloured since is left alone: only
// exactly the recorded grey is replaced. No undo point; the project is marked
// changed, so the next save writes the real colours (and no ledger).
static void healPersistedTints() {
  if (g_tintHeal.empty()) return;
  std::map<std::string, TintLedgerEntry> heal;
  heal.swap(g_tintHeal);
  ReaProject* proj = std::exchange(g_tintHealProject, nullptr);
  if (g_mode != Mode::Idle && g_mode != Mode::Browsing) return;
  bool open = false;
  for (int i = 0; ReaProject* p = EnumProjects(i, nullptr, 0); ++i)
    if (p == proj) open = true;
  if (!proj || !open) return;
  int healed = 0;
  for (int i = 0, n = CountMediaItems(proj); i < n; ++i) {
    MediaItem* item = GetMediaItem(proj, i);
    auto e = heal.find(itemGuid(item));
    if (e == heal.end()) continue;
    if (static_cast<int>(GetMediaItemInfo_Value(item, "I_CUSTOMCOLOR")) != e->second.tint) continue;
    SetMediaItemInfo_Value(item, "I_CUSTOMCOLOR", static_cast<double>(e->second.real));
    ++healed;
  }
  if (healed == 0) return;
  MarkProjectDirty(proj);
  UpdateArrange();
  logEvent("tints_healed", {{"count", healed}},
           "restored the colour of " + std::to_string(healed) +
               " item(s) saved grey while another person was editing them");
}

// Reconcile the dark-grey tints against the live set of items another peer is
// editing. Newly-locked items get their real color stashed and are painted grey;
// released items are restored to that real color. The color writes are
// echo-suppressed and the snapshot is re-baselined to the tinted state, so the
// tint NEVER enters the capture diff. Runs on the main thread at the END of the
// Hosting/ClientActive tick: after remote applies, which restore the real color
// and drop the tint record (see applyItem), so a tinted item that was remotely
// recolored re-stashes its FRESH real color here and goes grey again. The
// restore color therefore tracks the live synced color, never a stale snapshot.
static void reconcileItemTints() {
  if (g_mode != Mode::Hosting && g_mode != Mode::ClientActive) return;
  // When the tint is switched off in the app, paint nothing (the empty desired
  // set restores every item to its real colour); the lock still binds: only
  // the visual cue is gone.
  std::set<std::string> desired = g_tintEnabled ? lockedItemsByOthers() : std::set<std::string>{};
  // Tint placeholders while their media is still downloading.
  if (g_tintEnabled) desired.insert(g_placeholders.begin(), g_placeholders.end());
  const int tint = lockTintNative();
  const auto now = std::chrono::steady_clock::now();
  bool changed = false;

  flushLocalEdits();
  g_applyingRemote = true;
  // Tint items newly locked by a peer (or freshly un-tinted by a remote apply,
  // which dropped their record so we re-stash the current real color), and
  // enforce the safety cap on a tint that has lingered too long.
  for (const std::string& guid : desired) {
    auto rec = g_itemTints.find(guid);
    if (rec == g_itemTints.end()) {
      MediaItem* it = findItemByGuid(guid);
      if (!it) continue;  // not local yet (e.g. its media is still downloading)
      g_itemTints[guid] = {static_cast<int>(GetMediaItemInfo_Value(it, "I_CUSTOMCOLOR")), now, false};
      SetMediaItemInfo_Value(it, "I_CUSTOMCOLOR", static_cast<double>(tint));
      changed = true;
    } else if (!rec->second.expired && now - rec->second.since > TINT_MAX_AGE &&
               !g_placeholders.count(guid)) {  // a placeholder stays grey until its media lands
      // Safety net: this tint has been up too long with no refreshing update, so
      // it is almost certainly stuck. Restore the real colour and mark it expired
      // so we don't immediately re-grey it; a genuine resume re-tints because the
      // remote update drops the record (see applyItem) and we re-stash fresh.
      if (MediaItem* it = findItemByGuid(guid))
        SetMediaItemInfo_Value(it, "I_CUSTOMCOLOR", static_cast<double>(rec->second.real));
      rec->second.expired = true;
      changed = true;
    }
  }
  // Restore items whose lock has released (an expired one already shows its real
  // colour, so just drop the record).
  for (auto it = g_itemTints.begin(); it != g_itemTints.end();) {
    if (desired.count(it->first)) {
      ++it;
      continue;
    }
    if (!it->second.expired)
      if (MediaItem* item = findItemByGuid(it->first))
        SetMediaItemInfo_Value(item, "I_CUSTOMCOLOR", static_cast<double>(it->second.real));
    it = g_itemTints.erase(it);
    changed = true;
  }

  if (changed) {
    UpdateArrange();
    // Re-baseline so the color writes are folded into the baseline and the next
    // capture diffs to nothing for these items. Mirrors applyRemoteChanges.
    g_snapshot = snapshotProject();
    g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  }
  g_applyingRemote = false;
}

// Keep locked and tinted items out of the capture diff.
//
// Items another peer holds are restored to their last synced state locally,
// without proposing an edit. Items in g_itemTints stay pinned to the baseline
// even after their lock expires, until the tint is removed. This closes the
// one-tick gap where an expired lock could let the grey color enter the diff.
//
// The local restore is echo-suppressed and re-baselines the change counter
// without an undo point. Keep g_itemTints intact so the real color remains
// available when the lock releases. Restore the owning track separately:
// the item chunk does not encode it, and a drag may have changed tracks.
static void revertLockedItemEdits(Snapshot& snap) {
  const std::set<std::string> locked = lockedItemsByOthers();
  if (locked.empty() && g_itemTints.empty()) return;

  // Everything to keep out of the diff: items a peer actively locks (snapped
  // back in REAPER too) plus anything we are currently tinting.
  std::set<std::string> pinned = locked;
  for (const auto& [guid, _] : g_itemTints) pinned.insert(guid);

  bool reverted = false;
  g_applyingRemote = true;
  for (const std::string& guid : pinned) {
    auto base = g_snapshot.items.find(guid);
    if (base == g_snapshot.items.end()) continue;  // not a synced item we baseline
    auto cur = snap.items.find(guid);
    const bool deleted = cur == snap.items.end();
    const bool moved = !deleted && cur->second.track != base->second.track;
    const bool edited = !deleted && cur->second.chunk != base->second.chunk;
    if (!deleted && !moved && !edited) continue;  // untouched this tick

    // Only an item a peer ACTIVELY locks gets its REAPER edit snapped back; a
    // merely-tinted (e.g. just-released) item is only pinned out of the diff so
    // its grey colour can't leak: the user's edit is theirs to keep.
    if (locked.count(guid)) {
      if (MediaItem* it = findItemByGuid(guid)) {
        // Put it back on its synced track first (setObjectStateChunk rebuilds the
        // item but never moves it across tracks), then restore the chunk. This
        // does NOT go through applyItem, so the tint record (real colour) survives.
        if (moved)
          if (MediaTrack* dst = findTrackByGuid(base->second.track))
            if (GetMediaItem_Track(it) != dst) MoveMediaItemToTrack(it, dst);
        if (MediaItem* it2 = findItemByGuid(guid))
          setObjectStateChunk(it2, base->second.chunk);
      } else {
        // The user deleted a locked item: re-create it from the synced state.
        // applyItem drops the tint record, so restore it afterward (same real
        // colour): the re-created item already carries the tint colour from the
        // baseline chunk, so reconcileItemTints then leaves it tinted.
        const int real = g_itemTints.count(guid) ? g_itemTints[guid].real : -1;
        applyItem(guid, json{{"present", true},
                             {"track", base->second.track},
                             {"chunk", base->second.chunk}});
        if (real != -1 && g_tintEnabled)
          g_itemTints[guid] = {real, std::chrono::steady_clock::now(), false};
      }
      reverted = true;
    }
    snap.items[guid] = base->second;  // pin: the capture diff proposes nothing for it
  }
  if (reverted) {
    UpdateArrange();
    g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  }
  g_applyingRemote = false;
}

// ---------- pings, follow mode, session meta, verified open ----------

// Parse "#rrggbb" to a native REAPER color int (with the custom-color flag),
// 0 when the string isn't a color. Local display only: never on the wire.
static int colorFromHex(const std::string& hex) {
  if (hex.size() != 7 || hex[0] != '#') return 0;
  char* end = nullptr;
  const long v = strtol(hex.c_str() + 1, &end, 16);
  if (!end || *end) return 0;
  return 0x1000000 | ColorToNative((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF);
}

// This user's own roster entry (peers[] is self-first), for name/color.
static json selfPeerEntry() {
  json peers = handleJson(reamesh_peers, json::array());
  if (peers.is_array())
    for (const auto& p : peers)
      if (p.is_object() && jsonBool(p, "is_self", false)) return p;
  return json::object();
}

// ---------- pings ----------
//
// A ping points everyone at one spot on the timeline ("look here") without
// touching anyone's project: the HUD shows it for a few seconds (who, what,
// where) with a click to jump there and one to keep it as a normal, synced
// marker (`add_marker`). On the wire it is still the 'M' shared-marker message,
// so an older peer receiving one inserts its local "⚑" marker as before.
struct Ping {
  uint64_t id = 0;
  long long at = 0;  // unix ms; the HUD times the notice from it
  std::string name, by, color, label;
  double pos = 0.0;
  bool self = false;
};
static std::vector<Ping> g_pings;
static uint64_t g_pingSeq = 0;
// The HUD shows a ping for ~8s; the extension keeps it a little longer so a
// HUD that (re)connects in the meantime still sees it, and only a few at once.
constexpr long long kPingKeepMs = 15000;
constexpr size_t kPingMax = 5;

static long long unixMsNow() {
  using namespace std::chrono;
  return duration_cast<milliseconds>(system_clock::now().time_since_epoch()).count();
}

static void addPing(const std::string& name, double pos, const std::string& by,
                    const std::string& color, bool self) {
  Ping p;
  p.id = ++g_pingSeq;
  p.at = unixMsNow();
  p.name = name;
  p.by = by;
  p.color = color;
  p.pos = pos;
  p.self = self;
  // In this user's own ruler format (bars/beats, time, ...), as REAPER shows it.
  char buf[64] = "";
  format_timestr_pos(pos, buf, sizeof(buf), -1);
  p.label = buf;
  g_pings.push_back(std::move(p));
  if (g_pings.size() > kPingMax) g_pings.erase(g_pings.begin());
}

// Recent pings for the ctl state; drops expired ones, and all of them outside
// a session.
static json pingsJson() {
  if (g_mode != Mode::Hosting && g_mode != Mode::ClientActive) g_pings.clear();
  const long long now = unixMsNow();
  g_pings.erase(std::remove_if(g_pings.begin(), g_pings.end(),
                               [now](const Ping& p) { return now - p.at > kPingKeepMs; }),
                g_pings.end());
  json out = json::array();
  for (const auto& p : g_pings)
    out.push_back({{"id", p.id}, {"at", p.at}, {"name", p.name}, {"by", p.by},
                   {"color", p.color}, {"pos", p.pos}, {"label", p.label},
                   {"self", p.self}});
  return out;
}

// Ping the current edit cursor to every peer. Our own HUD shows it too, as the
// confirmation that it went out (gossip never loops back).
static void sharePing(const std::string& rawName) {
  if ((g_mode != Mode::Hosting && g_mode != Mode::ClientActive) || !g_handle) return;
  std::string name = sanitizeMarkerName(rawName.c_str());
  if (name.size() > 200) name.resize(200);
  if (name.empty()) return;
  const double pos = GetCursorPosition();
  const json m = {{"name", name}, {"pos", pos}};
  if (!reamesh_share_marker(g_handle, m.dump().c_str())) return;
  const json self = selfPeerEntry();
  addPing(name, pos, jsonStr(self, "name"), jsonStr(self, "color"), true);
  logEvent("ping_sent", {{"name", name}}, "pinged \"" + name + "\"");
}

// Pings received from peers since the last tick.
static void drainPings() {
  if (!g_handle || (g_mode != Mode::Hosting && g_mode != Mode::ClientActive)) return;
  char* raw = reamesh_drain_markers(g_handle);
  std::string payload = raw ? raw : "[]";
  reamesh_free_string(raw);
  if (payload == "[]") return;
  json arr = json::parse(payload, nullptr, false);
  if (!arr.is_array() || arr.empty()) return;
  for (const auto& m : arr) {
    if (!m.is_object()) continue;
    const std::string name = sanitizeMarkerName(jsonStr(m, "name").c_str());
    const std::string by = jsonStr(m, "by");
    if (name.empty()) continue;
    addPing(name, jsonNum(m, "pos", 0.0), by, jsonStr(m, "color"), false);
    logEvent("ping_received", {{"name", name}, {"by", by}},
             by.empty() ? "ping \"" + name + "\"" : by + " pinged \"" + name + "\"");
  }
}

// A ping's "jump there": move the edit cursor and scroll the arrange view to
// it (a rolling transport follows, like click-to-follow).
static void jumpTo(double pos) {
  if (g_mode != Mode::Hosting && g_mode != Mode::ClientActive) return;
  if (!std::isfinite(pos) || pos < 0) return;
  SetEditCurPos(pos, true, true);
  UpdateArrange();
}

// A ping's "keep as marker": a normal project marker in the pinger's color,
// so it syncs to everyone like any other marker. An ordinary local edit, with
// an undo point of its own.
static void addMarkerAt(const std::string& rawName, double pos, const std::string& colorHex) {
  if (g_mode != Mode::Hosting && g_mode != Mode::ClientActive) return;
  if (!std::isfinite(pos) || pos < 0) return;
  std::string name = sanitizeMarkerName(rawName.c_str());
  // The "⚑ " prefix would opt it out of sync, the opposite of the point.
  while (isSharedMarkerName(name.c_str())) name.erase(0, sizeof(kSharedMarkerPrefix) - 1);
  if (name.size() > 200) name.resize(200);
  AddProjectMarker2(nullptr, false, pos, 0, name.c_str(), -1, colorFromHex(colorHex));
  Undo_OnStateChange("ReaMesh: add marker");
  UpdateArrange();
}

// Follow mode: mirror the followed peer's transport and view. Runs every
// timer tick (~30Hz) while a target is set, so the receiving end reacts to a
// beacon within one tick of it landing. The followed peer sees our beacon's
// `follow` field and switches to the fast beacon cadence (~100ms, see
// PRESENCE_FAST_INTERVAL in lib.rs), so play starts/stops and cursor moves
// mirror within ~150ms end to end. The beacon's play position is dated: the
// roster reports how long ago it landed (`age`) and the smoothed ping RTT, and
// while the target rolls both are added back so we seek to where their
// transport is *now*, not where it was when the beacon left. A local pause is
// overridden (it starts again: following means following), and only real
// drift (> kFollowDriftSec, at most once a second) re-seeks a rolling
// transport, since a re-seek is audible.
static void followMirrorSelection(const json& target);
static void followMirrorFx(const json& target);

static void followTick() {
  if (g_followPeer.empty()) return;
  json peers = handleJson(reamesh_peers, json::array());
  const json* target = nullptr;
  if (peers.is_array())
    for (const auto& p : peers)
      if (p.is_object() && jsonStr(p, "id") == g_followPeer) {
        target = &p;
        break;
      }
  if (!target) {
    // The peer left (or went stale): stop following rather than freezing on
    // their last known state.
    g_followPeer.clear();
    activityEvent("follow_ended", json::object(), "you stopped following because they left the session");
    return;
  }
  const bool tPlaying = target->value("playing", false);
  const double tPlay = target->value("play", 0.0);
  const std::string tFocus = jsonStr(*target, "focus");
  const double tv0 = target->value("v0", 0.0), tv1 = target->value("v1", 0.0);
  const bool playing = (GetPlayState() & 1) != 0;
  // Where their transport should be by now: the beacon position plus the time
  // it has spent in flight (half the round trip) and sitting here (`age`).
  // Only meaningful while they roll; a stopped cursor doesn't age.
  const double ageSec = target->value("age", 0.0) / 1000.0;
  const double rttSec = target->value("rtt", 0.0) / 1000.0;
  const double tNow = tPlaying ? tPlay + ageSec + rttSec * 0.5 : tPlay;
  const auto now = std::chrono::steady_clock::now();

  if (tPlaying && !playing) {
    // They are rolling and we are not (they just hit play, or the local user
    // tried to pause): jump to where they are now and roll.
    SetEditCurPos(tNow, true, true);
    Main_OnCommand(1007, 0);  // Transport: Play
    g_followLastSeek = now;
  } else if (!tPlaying && playing) {
    Main_OnCommand(1016, 0);  // Transport: Stop
    SetEditCurPos(tPlay, true, false);
  } else if (tPlaying && playing) {
    if (std::abs(GetPlayPosition() - tNow) > kFollowDriftSec &&
        now - g_followLastSeek > std::chrono::seconds(1)) {
      SetEditCurPos(tNow, false, true);
      g_followLastSeek = now;
    }
  } else if (g_followLastPlay < 0 || std::abs(tPlay - g_followLastPlay) > 1e-9) {
    // Both stopped: mirror their edit cursor as it moves.
    SetEditCurPos(tPlay, true, false);
  }
  g_followLastPlay = tPlay;

  if (!tFocus.empty() && tFocus != g_followLastFocus) {
    if (MediaTrack* tr = findTrackByGuid(tFocus)) {
      SetOnlyTrackSelected(tr);
      Main_OnCommand(40913, 0);  // vertical scroll selected track into view
    }
    g_followLastFocus = tFocus;
  }
  if (tv1 > tv0 &&
      (std::abs(tv0 - g_followV0) > 1e-6 || std::abs(tv1 - g_followV1) > 1e-6)) {
    double ns = tv0, ne = tv1;
    GetSet_ArrangeView2(nullptr, true, 0, 0, &ns, &ne);
    g_followV0 = tv0;
    g_followV1 = tv1;
  }
  followMirrorSelection(*target);
  followMirrorFx(*target);
}

// Mirror the followed peer's item selection: their beacon carries the GUIDs
// of the items they have selected, so what they are about to edit is what
// the follower sees highlighted. Applied only when the list changes (the
// follower's own clicks stand until the followed peer selects again), and
// only for items that exist locally: a GUID the follower doesn't have yet
// (media still streaming in, an item deferred on missing media) is skipped.
// Selection is per-user state and never captured, so this can't sync back.
static void followMirrorSelection(const json& target) {
  const json* sel = target.contains("sel") && target["sel"].is_array() ? &target["sel"] : nullptr;
  std::string key;
  if (sel)
    for (const auto& g : *sel)
      if (g.is_string()) key += g.get<std::string>() + '\n';
  if (key == g_followLastSel) return;
  g_followLastSel = key;
  std::unordered_set<std::string> want;
  if (sel)
    for (const auto& g : *sel)
      if (g.is_string()) want.insert(g.get<std::string>());
  PreventUIRefresh(1);
  const int n = CountMediaItems(nullptr);
  for (int i = 0; i < n; ++i) {
    MediaItem* item = GetMediaItem(nullptr, i);
    SetMediaItemSelected(item, want.count(itemGuid(item)) > 0);
  }
  PreventUIRefresh(-1);
  UpdateArrange();
}

static FollowFx parseFollowFx(const json& fx) {
  FollowFx f;
  if (!fx.is_object()) return f;
  f.track = jsonStr(fx, "track");
  if (f.track.empty()) return f;
  f.item = jsonStr(fx, "item");
  f.take = fx.contains("take") && fx["take"].is_number_integer() ? fx["take"].get<int>() : 0;
  f.fx = fx.contains("fx") && fx["fx"].is_number_integer() ? fx["fx"].get<int>() : 0;
  f.flt = fx.contains("float") && fx["float"].is_boolean() && fx["float"].get<bool>();
  f.valid = f.take >= 0 && f.fx >= 0;
  return f;
}

// Show (open the chain window on that effect, or float it) or hide the FX
// window a beacon describes. Returns whether the target FX resolved locally.
// A plain index is bounds-checked against the local chain; an index carrying
// REAPER's input-FX / container bits is passed through as REAPER encodes it.
static bool followFxShow(const FollowFx& f, bool show) {
  if (!f.valid) return false;
  const bool plain = (f.fx & 0x3000000) == 0;
  const int flag = show ? (f.flt ? 3 : 1) : (f.flt ? 2 : 0);
  MediaTrack* tr = findTrackByGuid(f.track);
  if (!tr) return false;
  if (f.item.empty()) {
    if (plain && f.fx >= TrackFX_GetCount(tr)) return false;
    TrackFX_Show(tr, f.fx, flag);
    return true;
  }
  MediaItem* item = findItemByGuid(f.item);
  if (!item) return false;
  MediaItem_Take* take = GetMediaItemTake(item, f.take);
  if (!take) return false;
  if (plain && f.fx >= TakeFX_GetCount(take)) return false;
  TakeFX_Show(take, f.fx, flag);
  return true;
}

// Mirror the FX window the followed peer is editing in (their beacon's `fx`:
// the focused FX, whether it sits in the chain window or floats). Opening the
// same effect here shows *what* they are working on, not just where. When
// they switch effects inside one chain window the local chain window just
// selects the new effect (no close/reopen flicker); a move to another
// track/take or to a floating window closes what follow mode opened first,
// and a beacon without `fx` (they closed the window) closes it too. A no-FX
// join has no effects to show, so it only ever mirrors the selection.
static void followMirrorFx(const json& target) {
  const json fx = target.contains("fx") && target["fx"].is_object() ? target["fx"] : json();
  const std::string key = fx.is_null() ? std::string() : fx.dump();
  if (key == g_followLastFx) return;
  g_followLastFx = key;
  if (g_noFx) return;
  const FollowFx next = parseFollowFx(fx);
  const bool sameChain = next.valid && g_followFxOpen.valid && !next.flt && !g_followFxOpen.flt &&
                         next.track == g_followFxOpen.track && next.item == g_followFxOpen.item &&
                         next.take == g_followFxOpen.take;
  if (!sameChain) followFxShow(g_followFxOpen, false);
  g_followFxOpen = FollowFx{};
  if (followFxShow(next, true)) g_followFxOpen = next;
}

static void stopFollowing() {
  g_followPeer.clear();
  g_followLastFocus.clear();
  g_followLastPlay = -1.0;
  g_followV0 = g_followV1 = 0.0;
  g_followLastSeek = {};
  g_followLastSel.clear();
  g_followLastFx.clear();
  // Whatever follow mode opened stays open: the user keeps the view they had
  // when they stopped, exactly like the track focus and arrange span do.
  g_followFxOpen = FollowFx{};
}

// Read a whole file as UTF-8 text ("" on failure).
static std::string readFileText(const std::string& path) {
  std::ifstream f(fsPath(path), std::ios::binary);
  if (!f) return "";
  return std::string((std::istreambuf_iterator<char>(f)), std::istreambuf_iterator<char>());
}

// The hosted session's meta: track count + the plugin inventory of the .rpp
// actually being served. Recomputed when that file changes (a mid-session
// save), so a late joiner's preview matches what they will download.
static void refreshSessionMeta(bool force) {
  if (g_mode != Mode::Hosting || !g_handle || g_sessionProjectPath.empty()) return;
  const auto now = std::chrono::steady_clock::now();
  if (!force && now - g_lastMetaCheck < std::chrono::seconds(2)) return;
  g_lastMetaCheck = now;
  std::error_code ec;
  const auto mtime = fs::last_write_time(fsPath(g_sessionProjectPath), ec);
  if (ec || (!force && mtime == g_metaRppMtime)) return;
  g_metaRppMtime = mtime;
  const std::string text = readFileText(g_sessionProjectPath);
  if (text.empty()) return;
  json plugins = json::array();
  for (const std::string& p : collectPlugins(text)) plugins.push_back(p);
  // `reaper` is this host's REAPER version: a joiner on a different one is
  // warned in the trust dialog (a newer host's project may not load faithfully).
  const json meta = {{"v", 1},
                     {"tracks", CountTracks(nullptr)},
                     {"plugins", plugins},
                     {"reaper", g_reaperVersion}};
  reamesh_set_session_meta(g_handle, meta.dump().c_str());
}

// Compare the received .rpp against what the host's manifest claimed it loads.
// Null means the project is clean and may open immediately. Otherwise return
// {"extras":[names...],"listed":bool}. If `listed` is false, every
// plugin found counts as unlisted (a host must not dodge verification by
// simply omitting the claim).
static json verifyDownloadedPlugins(const std::string& path) {
  const std::string text = readFileText(path);
  if (text.empty()) return json();
  const std::vector<std::string> found = collectPlugins(text);
  if (found.empty()) return json();
  std::set<std::string> claimed;
  bool listed = false;
  if (g_handle) {
    char* raw = reamesh_session_meta(g_handle);
    const std::string metaStr = raw ? raw : "";
    reamesh_free_string(raw);
    const json meta = json::parse(metaStr, nullptr, false);
    if (meta.is_object() && meta.contains("plugins") && meta["plugins"].is_array()) {
      listed = true;
      for (const auto& p : meta["plugins"])
        if (p.is_string()) claimed.insert(p.get<std::string>());
    }
  }
  json extras = json::array();
  for (const std::string& f : found)
    if (!claimed.count(f)) extras.push_back(f);
  if (extras.empty()) return json();
  return json{{"extras", extras}, {"listed", listed}};
}

// The host's manifest meta names its REAPER version. A different one here is
// worth a line in the Activity feed (the trust dialog already warned before the
// join): state chunks are REAPER's own serialization, so a field written by a
// newer REAPER can come back changed from an older one. An old host that sends
// no version says nothing.
static void noteHostReaperMismatch() {
  if (!g_handle || g_reaperVersion.empty()) return;
  char* raw = reamesh_session_meta(g_handle);
  const json meta = json::parse(raw ? raw : "", nullptr, false);
  reamesh_free_string(raw);
  const std::string host = meta.is_object() ? jsonStr(meta, "reaper") : "";
  if (host.empty() || reaperVersionKey(host) == reaperVersionKey(g_reaperVersion)) return;
  logEvent("reaper_mismatch", {{"host", host}, {"local", g_reaperVersion}},
           "the host runs REAPER " + host + ", you run " + g_reaperVersion +
               ". Some edits may not sync cleanly");
}

// Tear the session down. `code`/`params`/`text` describe why as a structured
// log event (also flagged to the app as the error); empty text = silent.
static void leaveSession(const char* code, json params, const std::string& text) {
  // Repaint any locked-item tints back to their real colors before we drop the
  // session, so the user's still-open project is never left with grey items
  // (and a later save can't persist the tint). Runs while still in-session so
  // the suppress + re-baseline is correct; it's a no-op when nothing is tinted.
  restoreAllItemTints();
  if (g_handle) {
    reamesh_shutdown(g_handle);
    g_handle = nullptr;
  }
  g_mode = Mode::Idle;
  g_sessionProjectPath.clear();
  g_snapshot = Snapshot{};
  g_deferredItems.clear();
  g_deferredRecv.clear();
  g_placeholders.clear();
  g_peakJobs.clear();  // REAPER builds whatever is left on demand
  g_mediaLoading = false;
  g_downloadBackground = false;
  g_placeholdersGivenUp = false;
  g_placeholderGraceSince = {};
  g_lastChangeCount = -1;
  g_groupNamesAt = -1;  // a new project invalidates the cached group names
  g_lastPeerCount = 0;
  g_lastReverts = 0;
  g_bootstrapSynced = false;
  stopFollowing();
  g_pendingVerify = json();
  g_pendingOpenPath.clear();
  // g_noFx deliberately survives a leave: an auto-rejoin must come back in the
  // same mode the user chose. A fresh host/join sets it explicitly.
  if (!text.empty()) g_lastError = logEvent(code, std::move(params), text);
}

// Start LAN session discovery (Idle -> Browsing). Idempotent while browsing.
static bool startBrowse() {
  if (g_mode == Mode::Browsing) return true;
  if (g_mode != Mode::Idle) return false;
  std::string staging = std::string(GetResourcePath()) + "/ReaMesh";
  // Every project open in this REAPER (all tabs), so the staging sweep can
  // never remove a downloaded copy that is still open here.
  std::string open;
  for (int i = 0;; ++i) {
    char buf[4096] = {0};
    if (!EnumProjects(i, buf, sizeof(buf))) break;
    if (buf[0]) {
      open += buf;
      open += '\n';
    }
  }
  g_handle = reamesh_join(staging.c_str(), open.c_str());
  if (!g_handle) {
    g_lastError = logEvent("browse_failed", json::object(), "could not start session discovery");
    return false;
  }
  g_mode = Mode::Browsing;
  g_lastError = json();
  logEvent("browsing", json::object(), "searching for sessions on the network");
  return true;
}

// Join a session (Browsing -> Downloading): one of the announced sessions by
// id, or (with a non-empty `ticket`) through a pasted invite ticket. For a
// ticket the Rust side dials the ticket's addresses and joins once the host's
// announce arrives over that connection; that is how a session outside
// mDNS/multicast reach (VPN, strict LAN) is joined.
static bool joinSession(const std::string& sid, const std::string& ticket,
                        const std::string& sname, const std::string& passphrase) {
  if (g_mode != Mode::Browsing || !g_handle) return false;
  const bool ok = ticket.empty()
                      ? reamesh_join_session(g_handle, sid.c_str(), passphrase.c_str())
                      : reamesh_join_ticket(g_handle, ticket.c_str(), passphrase.c_str());
  if (!ok) {
    // Flagged as the error, not just logged: the app's join button is
    // fire-and-forget, so this is the only thing that tells the user why it
    // came back (a double-click on a join already under way is guarded on the
    // app side, so a refusal here is a genuine one).
    g_lastError =
        ticket.empty()
            ? logEvent("join_unavailable", {{"name", sname}},
                       "could not join \"" + sname + "\" (it may have just closed); try again")
            : logEvent("bad_ticket", json::object(),
                       "that invite code doesn't look valid. Copy it again from the host");
    return false;
  }
  // Rust resets its cumulative revert counter for the new session; match it
  // here so the first real override is detected as a delta rather than masked
  // by a count left over from a previous session (e.g. across an auto-rejoin).
  g_lastReverts = 0;
  g_mode = Mode::Downloading;
  g_lastError = json();
  g_joinPassphrase = passphrase;
  g_joinTicket = ticket;
  logEvent("joining", {{"name", sname}}, "joining \"" + sname + "\". The project is downloading");
  return true;
}

// Recover from a host crash or network interruption. If the host returns, the
// session id remains valid and the staged download avoids duplicate transfers.
// Keep watching the lobby and reuse the passphrase for a bounded retry window.
static void beginReconnect(const json& st) {
  Reconnect rc;
  rc.sessionId = jsonStr(st, "session_id");
  rc.sessionName = jsonStr(st, "session_name");
  if (rc.sessionName.empty()) rc.sessionName = "session";
  rc.passphrase = g_joinPassphrase;
  rc.ticket = g_joinTicket;
  rc.deadline = std::chrono::steady_clock::now() + std::chrono::seconds(45);
  leaveSession("", json::object(), "");  // silent: this is not (yet) an error
  if (rc.sessionId.empty() || !startBrowse()) {
    g_lastError =
        logEvent("host_lost", json::object(), "lost the connection to the host");
    return;
  }
  logEvent("reconnecting", {{"name", rc.sessionName}},
           "connection to the host lost. Reconnecting to \"" + rc.sessionName + "\"");
  g_reconnect = std::move(rc);
}

// The session ended under us (status "ended"). A vanished host triggers an
// automatic rejoin attempt; everything else sends the user back to the lobby
// with a message that says why. Being kicked or banned is never retried:
// the host asked us to go (a kicked user can still rejoin by hand).
static void onSessionEnded(const json& st) {
  const std::string why = jsonStr(st, "end_reason");
  if (why == "host_lost") beginReconnect(st);
  else if (why == "kicked")
    leaveSession("kicked", json::object(), "the host removed you from the session");
  else if (why == "banned")
    leaveSession("banned", {{"minutes", 30}},
                 "the host blocked you from this session for 30 minutes");
  else leaveSession("host_ended", json::object(), "the host ended the session");
}

// Browsing-mode tick while an auto-rejoin is pending: rejoin as soon as the
// session is announced again, give up (back to the plain lobby) on deadline.
static void reconnectTick() {
  if (!g_reconnect) return;
  if (std::chrono::steady_clock::now() > g_reconnect->deadline) {
    g_lastError = logEvent("reconnect_failed", {{"name", g_reconnect->sessionName}},
                           "could not reconnect to \"" + g_reconnect->sessionName + "\"");
    g_reconnect.reset();
    return;
  }
  // Ticket-joined session: the host is usually beyond mDNS reach, so its
  // announce will never show up here: re-dial the ticket instead. The Rust
  // side keeps dialing + waiting for the pinned host for up to 30s on its own,
  // which covers a network blip; a host that actually re-hosted has a new
  // session id and can't be auto-rejoined either way.
  if (!g_reconnect->ticket.empty()) {
    Reconnect rc = *g_reconnect;
    g_reconnect.reset();
    joinSession("", rc.ticket, rc.sessionName, rc.passphrase);
    return;
  }
  json sessions = handleJson(reamesh_sessions, json::array());
  if (!sessions.is_array()) return;
  for (const auto& s : sessions) {
    if (s.is_object() && jsonStr(s, "session_id") == g_reconnect->sessionId) {
      Reconnect rc = *g_reconnect;
      g_reconnect.reset();
      joinSession(rc.sessionId, "", rc.sessionName, rc.passphrase);
      return;
    }
  }
}

// What hosting would silently do to this machine, computed BEFORE the host
// starts: internalizeExternalMedia will copy every out-of-project source into
// the project folder (potentially tens of GB, with no free-space check), and a
// project inside a cloud-synced folder means the sync client will fight the
// session over the very files peers are transferring. Null = nothing to warn
// about; otherwise {files,bytes,free,free_known,cloud,enough} for the app's
// confirm dialog.
static json hostPrecheck() {
  const std::string path = currentProjectPath();
  if (path.empty()) return json();  // unsaved: onHost's save-first flow handles it
  const fs::path projDir = fsPath(path).parent_path();

  // External media that hosting would copy in (dedup by source path).
  std::set<std::string> seen;
  uint64_t bytes = 0;
  int count = 0;
  const int n = CountMediaItems(nullptr);
  for (int i = 0; i < n; ++i) {
    MediaItem_Take* take = GetActiveTake(GetMediaItem(nullptr, i));
    if (!take) continue;
    PCM_source* src = GetMediaItemTake_Source(take);
    if (!src) continue;
    char fn[4096] = "";
    GetMediaSourceFileName(src, fn, sizeof(fn));
    if (!fn[0]) continue;
    std::error_code ec;
    const fs::path source = fsPath(fn);
    const fs::path rel = fs::relative(source, projDir, ec);
    const bool outside = !rel.empty() && *rel.begin() == fs::path("..");
    if (!ec && !rel.empty() && !outside) continue;  // already inside
    if (!seen.insert(fn).second) continue;
    ++count;
    std::error_code sec;
    const auto sz = fs::file_size(source, sec);
    if (!sec) bytes += (uint64_t)sz;
  }

  // Cloud-synced folders by their well-known path fragments.
  std::string lower = fsUtf8(projDir);
  std::transform(lower.begin(), lower.end(), lower.begin(),
                 [](unsigned char c) { return (char)tolower(c); });
  const bool cloud = lower.find("dropbox") != std::string::npos ||
                     lower.find("onedrive") != std::string::npos ||
                     lower.find("google drive") != std::string::npos ||
                     lower.find("googledrive") != std::string::npos ||
                     lower.find("com~apple~clouddocs") != std::string::npos ||
                     lower.find("mobile documents") != std::string::npos ||
                     lower.find("icloud") != std::string::npos;
  if (count == 0 && !cloud) return json();

  std::error_code ec;
  const fs::space_info sp = fs::space(projDir, ec);
  const uint64_t freeB = ec ? 0 : (uint64_t)sp.available;
  // Reserve 256 MiB of free space beyond the media copy size.
  const bool enough = ec ? true : bytes + (256ull << 20) <= freeB;
  return json{{"files", count},   {"bytes", bytes},   {"free", freeB},
              {"free_known", !ec}, {"cloud", cloud},   {"enough", enough}};
}

static void onHost(const std::string& passphrase) {
  if (g_mode == Mode::Hosting) return;
  if (g_mode != Mode::Idle) leaveSession("left", json::object(), "left previous session");
  g_noFx = false;  // hosting is always a full, read-write session

  std::string path = currentProjectPath();
  if (path.empty()) {
    // Unsaved project: trigger Save As so there is a project file to share.
    g_lastError =
        logEvent("save_first", json::object(), "save the project first, then host again");
    Main_SaveProject(nullptr, true);
    return;
  }
  Main_SaveProject(nullptr, false);  // share what's on screen, not a stale file

  size_t slash = path.find_last_of("/\\");
  std::string dir = (slash != std::string::npos) ? path.substr(0, slash) : ".";
  std::string file = (slash != std::string::npos) ? path.substr(slash + 1) : path;
  std::string name = fileStem(file);

  g_handle = reamesh_host(name.c_str(), dir.c_str(), file.c_str(), passphrase.c_str());
  if (!g_handle) {
    g_lastError = logEvent("host_failed", json::object(), "could not start hosting");
    return;
  }
  g_mode = Mode::Hosting;
  g_bootstrapSynced = true;  // The owner is already the source of truth.
  muteActivity(std::chrono::seconds(6));
  g_lastError = json();
  g_sessionProjectPath = path;
  g_snapshot = Snapshot{};
  g_itemTints.clear();  // fresh session: no carried-over lock tints
  g_lastChangeCount = -1;  // force a full capture; this seeds the shared doc
  g_lastCapture = {};
  g_lastFullScan = {};
  g_captureCost = {};  // new project: re-learn what a capture costs
  // Copies from a previous session may still finish (drained harmlessly by
  // path match); only the per-session verdicts reset.
  g_copyFailed.clear();
  g_itemsAwaitingCopy.clear();
  g_lastPeerCount = 0;
  logEvent("hosting", {{"name", name}},
           passphrase.empty()
               ? "hosting \"" + name + "\". Sharing all files in " + dir
               : "hosting \"" + name + "\" with a passphrase. Sharing " + dir);
  // Inventory the served .rpp (track count + plugin list) so joiners see what
  // a join loads before downloading; kept fresh as the file changes.
  g_metaRppMtime = {};
  refreshSessionMeta(true);
}

static void onLeave() {
  g_reconnect.reset();  // a manual leave also cancels a pending auto-rejoin
  if (g_mode == Mode::Idle) return;
  leaveSession("left", json::object(), "left the session");
  g_lastError = json();  // user-initiated, nothing to flag in the app
}

// ---------- JSON helper ----------

static std::string jsonStr(const json& j, const char* key) {
  auto it = j.find(key);
  return (it != j.end() && it->is_string()) ? it->get<std::string>() : "";
}
// Typed reads of peer-authored JSON. nlohmann's value<T>() throws on a
// mismatched type, and nothing in the apply path may throw on REAPER's timer.
static bool jsonBool(const json& j, const char* key, bool dflt) {
  auto it = j.find(key);
  return (it != j.end() && it->is_boolean()) ? it->get<bool>() : dflt;
}
static double jsonNum(const json& j, const char* key, double dflt) {
  auto it = j.find(key);
  return (it != j.end() && it->is_number()) ? it->get<double>() : dflt;
}

// ---------- app control server ----------
//
// The Rust lib runs a localhost WebSocket server (reamesh_ctl_*) that the
// ReaMesh app connects to. The app's commands are queued there and
// drained here, on the main thread, where it is safe to drive the
// browse/host/join/leave paths; in return we push one composed state JSON
// (mode, session, people with names/colors, announced sessions, activity
// log) whenever it changes. The server itself never touches the session.

static const char* modeName() {
  switch (g_mode) {
    case Mode::Hosting: return "hosting";
    case Mode::Browsing: return "browsing";
    case Mode::Downloading: return "downloading";
    case Mode::ClientActive: return "client";
    case Mode::Idle: break;
  }
  return "idle";
}

static json handleJson(char* (*fn)(ReameshHandle*), const json& fallback) {
  if (!g_handle) return fallback;
  char* raw = fn(g_handle);
  json parsed = json::parse(raw ? raw : "", nullptr, false);
  reamesh_free_string(raw);
  return parsed.is_discarded() ? fallback : parsed;
}

// Click-to-follow moves this REAPER view to the peer's focused track and
// playhead position. Resolve presence again here so stale UI state cannot move
// the view to an old location.
static void revealPeer(const std::string& peerId) {
  if (peerId.empty()) return;
  json peers = handleJson(reamesh_peers, json::array());
  if (!peers.is_array()) return;
  std::string guid;
  double play = 0.0, pv0 = 0.0, pv1 = 0.0;
  bool havePlay = false;
  for (const auto& p : peers) {
    if (p.is_object() && jsonStr(p, "id") == peerId) {
      guid = jsonStr(p, "focus");
      if (p.contains("play") && p["play"].is_number()) {
        play = p["play"].get<double>();
        havePlay = true;
      }
      if (p.contains("v0") && p["v0"].is_number()) pv0 = p["v0"].get<double>();
      if (p.contains("v1") && p["v1"].is_number()) pv1 = p["v1"].get<double>();
      break;
    }
  }
  // Vertical: bring their focused track on-screen (if we can resolve it).
  // "Track: Vertical scroll selected tracks into view": non-activating, so it
  // never steals focus from REAPER (the overlay panel doesn't activate).
  if (MediaTrack* tr = guid.empty() ? nullptr : findTrackByGuid(guid)) {
    SetOnlyTrackSelected(tr);
    Main_OnCommand(40913, 0);
  }
  // Jump the edit cursor to where they are (their play cursor while rolling,
  // else their edit cursor); seekplay=true carries a rolling transport along,
  // so "follow" means follow, not just glance.
  if (havePlay) SetEditCurPos(play, false, true);
  // Horizontal: adopt the peer's own arrange-view span when their beacon
  // carries one: the follower lands with the same zoom and scroll they see.
  // Fallback (older peer, no view yet): keep our zoom and centre their cursor.
  if (pv1 > pv0) {
    double ns = pv0, ne = pv1;
    GetSet_ArrangeView2(nullptr, true, 0, 0, &ns, &ne);
  } else if (havePlay) {
    double t0 = 0, t1 = 0;
    GetSet_ArrangeView2(nullptr, false, 0, 0, &t0, &t1);
    const double span = t1 - t0;
    if (span > 0) {
      double ns = play - span / 2.0;
      if (ns < 0) ns = 0;
      double ne = ns + span;
      GetSet_ArrangeView2(nullptr, true, 0, 0, &ns, &ne);
    }
  }
  UpdateArrange();
}

// Host-side moderation from the app's roster: kick (they may rejoin at once)
// or ban (their device is kept out for 30 minutes; see reamesh_bans). The
// name is resolved before the call because the peer is gone from the roster
// the moment the core accepts it.
static void moderatePeer(const std::string& peerId, bool ban) {
  if (peerId.empty() || g_mode != Mode::Hosting || !g_handle) return;
  std::string name;
  json peers = handleJson(reamesh_peers, json::array());
  if (peers.is_array())
    for (const auto& p : peers)
      if (p.is_object() && jsonStr(p, "id") == peerId) name = jsonStr(p, "name");
  if (!reamesh_moderate(g_handle, peerId.c_str(), ban)) return;
  const std::string who = name.empty() ? "a guest" : name;
  if (ban)
    logEvent("peer_banned", {{"name", name}}, "blocked " + who + " for 30 minutes");
  else
    logEvent("peer_kicked", {{"name", name}}, "removed " + who + " from the session");
}

static void unbanPeer(const std::string& peerId) {
  if (peerId.empty()) return;
  std::string name;
  if (char* b = reamesh_bans()) {
    json bans = json::parse(b, nullptr, false);
    reamesh_free_string(b);
    if (bans.is_array())
      for (const auto& e : bans)
        if (e.is_object() && jsonStr(e, "id") == peerId) name = jsonStr(e, "name");
  }
  if (!reamesh_unban(peerId.c_str())) return;
  logEvent("peer_unbanned", {{"name", name}},
           "unblocked " + (name.empty() ? std::string("a guest") : name));
}

static void ctlDispatch(const json& cmd) {
  const std::string c = jsonStr(cmd, "cmd");
  const auto cmdBool = [&](const char* key) {
    return cmd.contains(key) && cmd[key].is_boolean() && cmd[key].get<bool>();
  };
  // Any explicit user action supersedes a pending auto-rejoin.
  if (c == "host" || c == "join" || c == "leave") g_reconnect.reset();
  if (c == "browse") {
    startBrowse();
  } else if (c == "host") {
    g_hostWarning = json();
    if (!cmdBool("confirm")) {
      // First pass: if hosting would copy external media in (or the project
      // lives in a cloud-synced folder), surface the numbers and wait for the
      // app's confirm instead of silently filling the disk.
      json w = hostPrecheck();
      if (!w.is_null()) {
        g_hostWarning = std::move(w);
        return;
      }
    }
    onHost(jsonStr(cmd, "passphrase"));
  } else if (c == "host_dismiss") {
    g_hostWarning = json();
  } else if (c == "join") {
    if (g_mode == Mode::Idle) startBrowse();
    const std::string name = jsonStr(cmd, "session_name");
    // "Join without effects": strictly view-only, every FX chain stripped.
    // Adopted only once the join is under way: a join refused because one is
    // already in progress must not flip the flag of the session it refused for.
    if (joinSession(jsonStr(cmd, "session_id"), jsonStr(cmd, "ticket"),
                    name.empty() ? "session" : name, jsonStr(cmd, "passphrase")))
      g_noFx = cmdBool("nofx");
  } else if (c == "open_confirm") {
    // The user read the "loads plugins that weren't listed" warning and chose
    // to continue: open the held-back download now.
    if (!g_pendingOpenPath.empty() && g_mode == Mode::Downloading)
      openDownloadedProject(g_pendingOpenPath);
  } else if (c == "share_marker") {
    sharePing(jsonStr(cmd, "name"));
  } else if (c == "jump_to") {
    jumpTo(jsonNum(cmd, "pos", -1.0));
  } else if (c == "add_marker") {
    addMarkerAt(jsonStr(cmd, "name"), jsonNum(cmd, "pos", -1.0), jsonStr(cmd, "color"));
  } else if (c == "kick" || c == "ban") {
    moderatePeer(jsonStr(cmd, "peer_id"), c == "ban");
  } else if (c == "unban") {
    unbanPeer(jsonStr(cmd, "peer_id"));
  } else if (c == "follow") {
    // "" stops following. Following yourself is meaningless: ignore it.
    const std::string pid = jsonStr(cmd, "peer_id");
    if (pid.empty()) {
      stopFollowing();
    } else if (jsonStr(selfPeerEntry(), "id") != pid) {
      stopFollowing();
      g_followPeer = pid;
    }
  } else if (c == "check") {
    // Passphrase pre-check: verify with the owner BEFORE the app shows the
    // trust dialog or starts downloading. Stays in Browsing; the result rides
    // the status `probe` field. Never transitions the session. With a `ticket`
    // the check dials the ticket's host instead of an announced session.
    if (g_mode == Mode::Idle) startBrowse();
    if (g_handle && g_mode == Mode::Browsing) {
      std::string ticket = jsonStr(cmd, "ticket");
      if (!ticket.empty()) {
        reamesh_check_ticket(g_handle, ticket.c_str(), jsonStr(cmd, "passphrase").c_str());
      } else {
        reamesh_check_passphrase(g_handle, jsonStr(cmd, "session_id").c_str(),
                                 jsonStr(cmd, "passphrase").c_str());
      }
    }
  } else if (c == "hint") {
    // A nearby host's addresses, read by the app off its Bluetooth
    // advertisement: dial them so the host's signed lobby announce can arrive.
    // Only meaningful in the Lobby; the core filters to our own subnets and
    // rate-limits, and nothing shows up unless a real session answers.
    if (g_handle && g_mode == Mode::Browsing) {
      auto port = [&](const char* k) {
        const double p = jsonNum(cmd, k, 0);
        return (p >= 1 && p <= 65535) ? (int)p : 0;
      };
      json hint = {{"ips", cmd.contains("ips") && cmd["ips"].is_array() ? cmd["ips"] : json::array()},
                   {"tcp", port("tcp")},
                   {"quic", port("quic")}};
      reamesh_dial_hint(g_handle, hint.dump().c_str());
    }
  } else if (c == "leave") {
    onLeave();
  } else if (c == "reveal_peer") {
    revealPeer(jsonStr(cmd, "peer_id"));
  } else if (c == "set_tint") {
    // App overlay setting: whether to paint the dark-grey lock tint.
    const bool on = cmd.contains("on") && cmd["on"].is_boolean() ? cmd["on"].get<bool>() : true;
    if (on != g_tintEnabled) {
      g_tintEnabled = on;
      // Turning it off: drop the grey at once (reconcileItemTints would catch up
      // next tick anyway, but this is immediate). Turning it on: the next tick's
      // reconcile re-applies whatever is still locked.
      if (!on) restoreAllItemTints();
    }
  }
}

// Compose the extension's state and publish it to the ReaMesh app when it
// changed. Kept apart from the command drain (ctlTick) so a long main-thread
// operation can publish an interim state on its own: openDownloadedProject
// does, right before Main_openProject blocks, without dispatching queued
// commands from inside itself: a `leave` landing there would tear the session
// down underneath the rest of that function.
static void ctlPublishState() {
  json state;
  // Each peer's beacon carries the GUID of the track they're focused on;
  // resolve it against THIS instance's tracks so the roster shows a
  // locally-correct 1-based track number. The GUID is the shared identity;
  // the index is per-project, so the resolution must happen here, never on the
  // wire. The raw GUID is dropped from what the app sees.
  json peers = handleJson(reamesh_peers, json::array());
  if (peers.is_array()) {
    for (auto& p : peers) {
      if (!p.is_object()) continue;
      std::string guid = jsonStr(p, "focus");
      p.erase("focus");
      // The app never shows timeline positions, view spans or beacon ages;
      // keeping them would republish the whole state on every presence beacon
      // (ten a second for a followed peer) while someone scrolls or plays
      // (click-to-follow asks the extension directly via `reveal_peer`).
      // `follow` and `frozen` stay: the roster paints the "following you" and
      // "REAPER isn't responding" indicators from them.
      p.erase("play");
      p.erase("v0");
      p.erase("v1");
      p.erase("playing");
      p.erase("age");
      p.erase("sel");
      p.erase("fx");
      if (guid.empty()) continue;
      if (MediaTrack* tr = findTrackByGuid(guid)) {
        int n = static_cast<int>(GetMediaTrackInfo_Value(tr, "IP_TRACKNUMBER"));
        if (n > 0) p["track"] = n;
      }
    }
    // "Alex is editing Bass DI": name what each other person holds an editing
    // lock on, so the roster says it in words. One entry per holder (their
    // first lock is enough for a roster line), resolved locally like the track
    // number above: the wire carries GUIDs, names are per-project.
    if (g_mode == Mode::ClientActive || g_mode == Mode::Hosting) {
      json locks = handleJson(reamesh_locked_entities, json::array());
      if (locks.is_array() && !locks.empty()) {
        std::map<std::string, json> editing;
        for (const auto& e : locks) {
          if (!e.is_object()) continue;
          const std::string holder = jsonStr(e, "holder");
          if (holder.empty() || editing.count(holder)) continue;
          const std::string section = jsonStr(e, "section");
          const std::string id = jsonStr(e, "id");
          MediaTrack* tr = nullptr;
          if (section == "tracks") {
            tr = findTrackByGuid(id);
          } else if (section == "items") {
            if (MediaItem* it = findItemByGuid(id)) tr = GetMediaItem_Track(it);
          } else if (section == "project") {
            editing[holder] = {{"kind", "project"}, {"field", jsonStr(e, "field")}};
            continue;
          }
          if (!tr) continue;
          char buf[512] = "";
          GetSetMediaTrackInfo_String(tr, "P_NAME", buf, false);
          editing[holder] = {
              {"kind", "track"},
              {"name", std::string(buf)},
              {"n", static_cast<int>(GetMediaTrackInfo_Value(tr, "IP_TRACKNUMBER"))}};
        }
        for (auto& p : peers) {
          if (!p.is_object()) continue;
          auto it = editing.find(jsonStr(p, "id"));
          if (it != editing.end()) p["editing"] = it->second;
        }
      }
    }
  }
  json st = handleJson(reamesh_status, json::object());
  // An edit of this user's just snapped back because someone else holds the
  // soft editing lock on that field. This fires for the client whose proposal
  // lost AND for the owner the lock bound (the host isn't privileged). Surface
  // a calm "X is editing <track>" notice plus a cumulative counter the app
  // toasts on the increase.
  long reverts = (st.contains("reverts") && st["reverts"].is_number_integer())
                     ? st["reverts"].get<long>()
                     : 0;
  if (g_mode == Mode::ClientActive || g_mode == Mode::Hosting) {
    if (reverts > g_lastReverts) {
      const long delta = reverts - g_lastReverts;
      // The status carries a tail of recent (section,id,by) snap-backs; the last
      // `delta` are new. Name the track and who is holding it.
      std::vector<std::pair<std::string, std::string>> events;  // (track, holder)
      std::vector<std::string> seenIds;
      if (st.contains("reverted") && st["reverted"].is_array()) {
        const json& rv = st["reverted"];
        const long start = std::max<long>(0, static_cast<long>(rv.size()) - delta);
        for (long i = start; i < static_cast<long>(rv.size()); ++i) {
          const json& e = rv[i];
          if (!e.is_object() || jsonStr(e, "section") != "tracks") continue;
          std::string id = jsonStr(e, "id");
          if (std::find(seenIds.begin(), seenIds.end(), id) != seenIds.end()) continue;
          seenIds.push_back(id);
          events.emplace_back(trackNameByGuid(id), peerNameById(peers, jsonStr(e, "by")));
        }
      }
      // Route through activityEvent so a burst on the same track collapses to
      // one notice (de-dupes identical text within a few seconds).
      if (events.empty()) {
        activityEvent("edit_locked", {{"count", delta}},
                      "that edit was undone because someone else is editing the same thing");
      } else {
        for (const auto& [nm, by] : events) {
          json params = json::object();
          if (!nm.empty()) params["track"] = nm;
          if (!by.empty()) params["by"] = by;
          std::string text =
              by.empty()
                  ? (nm.empty() ? "your edit snapped back"
                                : "your edit to " + nm + " snapped back")
                  : (nm.empty() ? by + " is editing this" : by + " is editing " + nm);
          activityEvent("edit_locked", params, text);
        }
      }
      g_lastReverts = reverts;
    }
  } else {
    reverts = 0;
  }
  state["type"] = "state";
  state["mode"] = modeName();
  state["project"] = fileStem(currentProjectPath());
  // Full paths this REAPER is using, so the app's joined-projects list never
  // offers to trash one of them: every open project tab, the session's
  // project, and the folder a join is downloading into (before it is open).
  {
    json inUse = json::array();
    for (int i = 0;; ++i) {
      char buf[4096] = {0};
      if (!EnumProjects(i, buf, sizeof(buf))) break;
      if (buf[0]) inUse.push_back(std::string(buf));
    }
    if ((g_mode == Mode::Hosting || g_mode == Mode::ClientActive) && !g_sessionProjectPath.empty())
      inUse.push_back(g_sessionProjectPath);
    if (g_mode == Mode::Downloading || g_mode == Mode::ClientActive) {
      const std::string dir = jsonStr(st, "dir");
      if (!dir.empty()) inUse.push_back(dir);
    }
    state["in_use"] = std::move(inUse);
  }
  // This REAPER's version, for the app's host-version mismatch warning.
  state["reaper"] = g_reaperVersion;
  state["error"] = g_lastError;
  state["log"] = g_log;
  state["overridden"] = reverts;
  // Recent pings, for the HUD notice (see sharePing).
  state["pings"] = pingsJson();
  state["reconnecting"] = static_cast<bool>(g_reconnect);
  // Passphrase pre-check result ({session_id, ok|null} or null), passed through
  // so the Lobby can show "wrong password" inline without a session transition.
  state["probe"] = (st.contains("probe")) ? st["probe"] : json();
  state["state"] = jsonStr(st, "state");
  state["progress"] =
      (st.contains("progress") && st["progress"].is_number()) ? st["progress"].get<double>() : 0.0;
  // Byte counters behind the download progress; the app renders "412 MB /
  // 1.3 GB" plus a transfer rate from consecutive states.
  state["done_bytes"] = (st.contains("done_bytes") && st["done_bytes"].is_number())
                            ? st["done_bytes"].get<uint64_t>()
                            : 0;
  state["total_bytes"] = (st.contains("total_bytes") && st["total_bytes"].is_number())
                             ? st["total_bytes"].get<uint64_t>()
                             : 0;
  state["files_done"] = (st.contains("files_done") && st["files_done"].is_number())
                            ? st["files_done"].get<uint64_t>()
                            : 0;
  state["files_total"] = (st.contains("files_total") && st["files_total"].is_number())
                             ? st["files_total"].get<uint64_t>()
                             : 0;
  // Relative path of the file currently being fetched, or "" outside a download.
  state["current_file"] = jsonStr(st, "current_file");
  state["opening"] = g_openingProject;
  std::string session = jsonStr(st, "session_name");
  if (session.empty() && !g_sessionProjectPath.empty()) session = fileStem(g_sessionProjectPath);
  state["session"] = session;
  // Invite ticket for the hosted session ("" while addresses are still being
  // learned, and always "" when not hosting): the app shows a copy control.
  std::string ticket;
  if (g_mode == Mode::Hosting && g_handle) {
    if (char* t = reamesh_session_ticket(g_handle)) {
      ticket = t;
      reamesh_free_string(t);
    }
  }
  state["ticket"] = ticket;
  // Live ban table ([{id,name,expires_at}]): process-global, so it is shown
  // (and revocable) whether or not a session is running right now.
  {
    json bans = json::array();
    if (char* b = reamesh_bans()) {
      json parsed = json::parse(b, nullptr, false);
      reamesh_free_string(b);
      if (parsed.is_array()) bans = std::move(parsed);
    }
    state["bans"] = std::move(bans);
  }
  // Pre-host media-copy warning ({files,bytes,free,cloud,enough} or null):
  // the app confirms before re-sending host with confirm:true.
  state["host_warning"] = g_hostWarning;
  // Post-download plugin verification ({extras,listed} or null): the app
  // confirms (open_confirm) or leaves; the open is held meanwhile.
  state["verify"] = g_pendingVerify;
  // This client joined without effects (view-only).
  state["nofx"] = g_noFx;
  // Progressive join: still receiving the project (media streaming in the
  // background and/or placeholder items waiting for it). Edits snap back
  // meanwhile; the byte/file counters above keep moving.
  state["loading"] = g_mediaLoading;
  state["placeholders"] = g_placeholders.size();
  state["sessions"] = handleJson(reamesh_sessions, json::array());
  state["peers"] = std::move(peers);

  static std::string lastPublished;
  std::string out = state.dump();
  if (out != lastPublished) {
    lastPublished = out;
    reamesh_ctl_publish(out.c_str());
  }
}

static void ctlTick() {
  // Alive beacon for the control server's watchdog: when this stops beating
  // (a modal REAPER dialog blocks the main thread), the server tells the app
  // "REAPER isn't responding" so a stuck Downloading/Lobby screen explains
  // itself instead of looking hung.
  reamesh_ctl_heartbeat();
  for (;;) {
    char* raw = reamesh_ctl_poll();
    if (!raw) break;
    json cmd = json::parse(raw, nullptr, false);
    reamesh_free_string(raw);
    if (cmd.is_object()) ctlDispatch(cmd);
  }
  ctlPublishState();
}

// ---------- timer ----------

// Sample the presence detail followers mirror: the selected item GUIDs (capped
// at kPresenceSelMax) and the FX window this user is editing in. The focused
// FX is REAPER's own notion (GetFocusedFX2: the FX whose window last had
// focus, as long as it is still open), reported as track GUID + raw FX index,
// or item GUID + take + index for a take FX, plus whether it floats or sits
// in the chain window. Master-track FX are out of scope like the master track
// itself. Rate-limited to the fast beacon cadence.
static void samplePresenceDetail() {
  const auto now = std::chrono::steady_clock::now();
  if (now - g_presSampled < std::chrono::milliseconds(100)) return;
  g_presSampled = now;

  json sel = json::array();
  const int n = CountSelectedMediaItems(nullptr);
  for (int i = 0; i < n && sel.size() < kPresenceSelMax; ++i)
    if (MediaItem* item = GetSelectedMediaItem(nullptr, i)) sel.push_back(itemGuid(item));
  g_presSel = std::move(sel);

  json fx;
  int trackNo = 0, itemNo = -1, fxNo = 0;
  const int what = GetFocusedFX2(&trackNo, &itemNo, &fxNo) & 3;
  if (what && trackNo > 0) {
    if (MediaTrack* tr = GetTrack(nullptr, trackNo - 1)) {
      if (what == 1) {
        fx = {{"track", trackGuid(tr)},
              {"fx", fxNo},
              {"float", TrackFX_GetFloatingWindow(tr, fxNo) != nullptr}};
      } else if (itemNo >= 0) {
        if (MediaItem* item = GetTrackMediaItem(tr, itemNo)) {
          // Take FX index: low word = FX index in the chain, high word = take.
          const int takeIdx = (fxNo >> 16) & 0xFFFF, idx = fxNo & 0xFFFF;
          if (MediaItem_Take* take = GetMediaItemTake(item, takeIdx))
            fx = {{"track", trackGuid(tr)},
                  {"item", itemGuid(item)},
                  {"take", takeIdx},
                  {"fx", idx},
                  {"float", TakeFX_GetFloatingWindow(take, idx) != nullptr}};
        }
      }
    }
  }
  g_presFx = std::move(fx);
}

// Publish the focused track GUID and current timeline position with the user's
// presence. Peers resolve the GUID against their own project. Track and item
// selections stay local state (never synced); item locks are shown with the
// local grey tint instead. The selected items and the open FX window do ride
// the beacon, but only so a *follower* can mirror them.
static void pushLocalPresence() {
  MediaTrack* focusTr = GetSelectedTrack(nullptr, 0);
  std::string focus = focusTr ? trackGuid(focusTr) : std::string();

  // "Where they are" on the timeline: the play cursor while the transport rolls,
  // otherwise the edit cursor.
  const bool playing = (GetPlayState() & 1) != 0;  // bit 0: playing
  const double play = playing ? GetPlayPosition() : GetCursorPosition();

  // The arrange-view span rides along so click-to-follow on a peer can adopt
  // this user's zoom + scroll, not just centre on their cursor.
  double v0 = 0, v1 = 0;
  GetSet_ArrangeView2(nullptr, false, 0, 0, &v0, &v1);

  // The selection + FX-window detail belongs to the shared project only: while
  // browsing or downloading, whatever is open is the user's previous project.
  if (g_mode == Mode::Hosting || g_mode == Mode::ClientActive) {
    samplePresenceDetail();
  } else {
    g_presSel = json::array();
    g_presFx = json();
    g_presSampled = {};
  }
  json p = {{"focus", focus}, {"play", play},          {"v0", v0},
            {"v1", v1},       {"playing", playing},    {"follow", g_followPeer},
            {"sel", g_presSel}, {"fx", g_presFx}};
  // Deliberately UNCONDITIONAL (no change-gate): every call also proves the
  // main thread is alive, which is what keeps this peer's beacon from
  // reporting "frozen" while the user simply sits still.
  reamesh_set_presence(p.dump().c_str());
}

// Open the finished download in REAPER (Downloading -> ClientActive). In
// no-FX mode a stripped sibling copy is written and opened instead: the
// downloaded original stays byte-identical so this peer can still serve it to
// other joiners (swarming hashes must keep matching the manifest).
static void openDownloadedProject(std::string path) {
  g_pendingVerify = json();
  g_pendingOpenPath.clear();
  // Progressive join: the download reported ready with media still streaming
  // (status.background). Items whose media isn't here yet open as empty
  // placeholders; see the g_placeholders notes.
  const json st = handleJson(reamesh_status, json::object());
  const bool progressive = jsonBool(st, "background", false);
  g_downloadBackground = progressive;
  g_placeholders.clear();
  g_placeholdersGivenUp = false;
  g_placeholderGraceSince = {};
  std::string text;
  bool rewritten = false;
  if (g_noFx || progressive) text = readFileText(path);
  if (g_noFx && !text.empty()) {
    text = stripFxBlocks(text, /*takeFxOnly=*/false);
    rewritten = true;
  }
  if (progressive && !text.empty()) {
    const fs::path projDir = fsPath(path).parent_path();
    BlankResult r = blankMissingSources(text, [&](const std::string& f) {
      std::error_code ec;
      const fs::path fp = fsPath(f);
      return fs::exists(fp.is_absolute() ? fp : projDir / fp, ec);
    });
    if (!r.guids.empty()) {
      text = std::move(r.text);
      g_placeholders = std::move(r.guids);
      rewritten = true;
      logEvent("progressive_open", {{"count", g_placeholders.size()}},
               "opening the project now; " + std::to_string(g_placeholders.size()) +
                   " item(s) are waiting for their media");
    }
  }
  // What REAPER opens vs. what we keep on disk. The downloaded original must
  // stay byte-identical (this copy is served to other joiners and has to match
  // the manifest hashes), so a rewritten text goes to a sibling in no-FX mode
  // (where the project deliberately stays a view-only copy), and for a
  // progressive open is written over the real path just for the duration of
  // Main_openProject and restored right after: REAPER reads the file once,
  // and the project keeps its real name.
  std::string original;
  bool restore = false;
  if (rewritten && g_noFx) {
    const fs::path stripped = fsPath(path).parent_path() / ".reamesh-nofx.rpp";
    std::ofstream o(stripped, std::ios::binary | std::ios::trunc);
    if (o) {
      o << text;
      o.close();
      path = fsUtf8(stripped);
      logEvent("nofx_open", json::object(),
               "opening a view-only copy without effects. No third-party code is loaded");
    } else {
      g_placeholders.clear();  // opening the original: nothing stands in
    }
  } else if (rewritten) {
    original = readFileText(path);
    std::ofstream o(fsPath(path), std::ios::binary | std::ios::trunc);
    if (o && !original.empty()) {
      o << text;
      o.close();
      restore = true;
    } else {
      g_placeholders.clear();  // couldn't rewrite: REAPER opens the original as before
    }
  }
  logEvent("download_complete", json::object(), "download complete. Opening " + path);
  // Main_openProject blocks the main thread for seconds on a big project and
  // the next state publish only happens afterwards (mode=client): publish
  // the "opening" phase first so the app's stepper doesn't sit frozen on the
  // download bar meanwhile.
  g_openingProject = true;
  // Publish only (no command drain): a `leave` dispatched from here would tear
  // the session down underneath the rest of this function.
  ctlPublishState();
  g_applyingRemote = true;
  Main_openProject(path.c_str());
  if (restore) {
    std::ofstream o(fsPath(path), std::ios::binary | std::ios::trunc);
    if (o) o << original;
  }
  // The loaded project IS the owner's state; baseline it so the load is
  // never diffed as a burst of local edits.
  g_groupNamesAt = -1;  // different project: the cached group names are stale
  g_snapshot = snapshotProject();
  g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  g_itemTints.clear();  // fresh download: start with no lock tints
  g_captureCost = {};   // new project: re-learn what a capture costs
  g_copyFailed.clear();
  g_itemsAwaitingCopy.clear();
  g_applyingRemote = false;
  g_openingProject = false;
  // Peak caches (.reapeaks) are deliberately not transferred, and REAPER
  // does not always rebuild them on its own (e.g. with a central peaks
  // folder, or "build peaks on the fly" off): items then render as flat
  // blocks. Queue the missing ones now; the build runs in the background.
  Main_OnCommand(40047, 0);  // Peaks: Build any missing peaks
  g_mode = Mode::ClientActive;
  // Don't propose anything until the owner's authoritative state has applied
  // and re-baselined us; and don't narrate that settle (the whole project
  // arriving as one changeset) as a flurry of "a track was added" events.
  g_bootstrapSynced = false;
  g_clientActiveSince = std::chrono::steady_clock::now();
  g_mediaLoading = progressive || !g_placeholders.empty();
  muteActivity(std::chrono::seconds(6));
  g_sessionProjectPath = currentProjectPath();
  logEvent("connected", json::object(),
           g_noFx ? "connected without effects in view-only mode"
                  : "connected. You can edit live, but the session owner wins conflicts");
  noteHostReaperMismatch();
}

static void pollDownload() {
  char* raw = reamesh_status(g_handle);
  std::string payload = raw ? raw : "{}";
  reamesh_free_string(raw);
  json st = json::parse(payload, nullptr, false);
  if (!st.is_object()) return;
  std::string state = st.value("state", "");

  if (state == "ready") {
    // Already holding for the user's verify answer: do nothing until the app
    // confirms (open_confirm) or the user leaves.
    if (!g_pendingOpenPath.empty()) return;
    std::string path = st.value("project_path", "");
    // In no-FX mode every chain is stripped before the open: nothing
    // third-party loads, so there is nothing to verify against the claim.
    if (!g_noFx) {
      json verify = verifyDownloadedPlugins(path);
      if (!verify.is_null()) {
        g_pendingOpenPath = path;
        g_pendingVerify = std::move(verify);
        logEvent("verify_hold", {{"count", g_pendingVerify["extras"].size()}},
                 "the downloaded project uses plugins missing from the host's list. Waiting for you");
        return;
      }
    }
    openDownloadedProject(path);
  } else if (state == "failed") {
    std::string msg = st.value("message", std::string("unknown error"));
    // Every routine failure gets a stable code so the app can render it in the
    // user's language. Without one, the raw English status text is spliced into
    // the localized sentence and a German user reads
    // "Beitritt fehlgeschlagen: manifest timeout". `join_failed` with a
    // {reason} stays as the last-resort fallback for anything genuinely
    // unexpected.
    if (msg.find("banned from this session") != std::string::npos)
      leaveSession("banned", {{"minutes", 30}},
                   "the host has blocked you from this session");
    else if (msg.find("version mismatch") != std::string::npos)
      leaveSession("version_mismatch", json::object(),
                   "the host runs a different ReaMesh version");
    else if (msg.find("could not prove the session passphrase") != std::string::npos)
      leaveSession("host_unverified", json::object(),
                   "the host could not prove it knows the session password");
    else if (msg.find("passphrase") != std::string::npos)
      leaveSession("wrong_password", json::object(), "join failed: " + msg);
    else if (msg.find("ticket host unreachable") != std::string::npos)
      leaveSession("ticket_unreachable", json::object(),
                   "couldn't reach the host from that invite code");
    else if (msg.find("manifest") != std::string::npos)
      leaveSession("manifest_timeout", json::object(),
                   "the host never sent the project file list");
    else if (msg.find("stall") != std::string::npos)
      leaveSession("download_stalled", json::object(),
                   "the download stopped partway through");
    else if (msg.find("no longer announced") != std::string::npos ||
             msg.find("owner") != std::string::npos)
      leaveSession("host_unreachable", json::object(), "couldn't reach the host");
    else
      leaveSession("join_failed", {{"reason", msg}}, "join failed: " + msg);
  } else if (state == "ended") {
    // The host went away while we were still downloading. Deliberate stop →
    // leave; vanished without a farewell → try to rejoin automatically (the
    // partial download dedups, so the retry is cheap).
    onSessionEnded(st);
  }
}

// Progressive join: track whether this client is still receiving its project
// (background download running, or placeholders waiting for media) and flip
// the editing gate accordingly. Placeholders that outlive the download by
// PLACEHOLDER_GRACE (their media never came) stop holding the gate but stay
// pinned out of the sync for good.
static void updateLoadingState(const json& st) {
  const bool background = jsonBool(st, "background", false);
  if (g_downloadBackground && !background)
    logEvent("media_downloaded", json::object(), "all media files are downloaded");
  g_downloadBackground = background;
  const auto now = std::chrono::steady_clock::now();
  bool waiting = !g_placeholders.empty() && !g_placeholdersGivenUp;
  if (waiting && !background) {
    if (g_placeholderGraceSince == std::chrono::steady_clock::time_point{})
      g_placeholderGraceSince = now;
    else if (now - g_placeholderGraceSince > PLACEHOLDER_GRACE) {
      g_placeholdersGivenUp = true;
      waiting = false;
      logEvent("placeholders_unresolved", {{"count", g_placeholders.size()}},
               std::to_string(g_placeholders.size()) +
                   " item(s) never received their media and stay empty here");
    }
  } else {
    g_placeholderGraceSince = {};
  }
  const bool loading = background || waiting;
  if (g_mediaLoading && !loading)
    logEvent("media_ready", json::object(), "everything is here. You can edit now");
  g_mediaLoading = loading;
}

// The whole extension on a REAPER older than kMinReaperVersion: keep the control
// server answering so the app can say "update REAPER" instead of mistaking the
// extension for missing, and do nothing else. Commands are drained and dropped,
// and no REAPER function is called: on a REAPER old enough to lack an import,
// those pointers are null.
static void unsupportedTick() {
  reamesh_ctl_heartbeat();
  while (char* raw = reamesh_ctl_poll()) reamesh_free_string(raw);
  static bool published = false;
  if (published) return;
  published = true;
  const json state = {{"type", "state"},
                      {"mode", "idle"},
                      {"state", ""},
                      {"progress", 0.0},
                      {"session", ""},
                      {"project", ""},
                      {"error", nullptr},
                      {"log", json::array()},
                      {"sessions", json::array()},
                      {"peers", json::array()},
                      {"bans", json::array()},
                      {"reaper", g_reaperVersion},
                      {"unsupported", {{"version", g_reaperVersion}, {"min", kMinReaperVersion}}}};
  reamesh_ctl_publish(state.dump().c_str());
}

static void timerTick() {
  if (g_unsupported) {
    unsupportedTick();
    return;
  }
  if (g_applyingRemote) return;
  // App commands and state mirroring run in every mode, including
  // Idle (the app may connect before any session exists).
  ctlTick();
  healPersistedTints();
  if (!g_handle) return;

  switch (g_mode) {
    case Mode::Idle:
      return;
    case Mode::Browsing:
      reconnectTick();
      return;
    case Mode::Downloading:
      // Keep the main-thread-alive presence feed running during the download
      // too: beacons already flow (the topic is joined at manifest time), and
      // without a touch this peer would read as "frozen" in everyone's roster.
      pushLocalPresence();
      pollDownload();
      return;
    case Mode::Hosting:
    case Mode::ClientActive: {
      // Switching projects ends the session: its doc is bound to the shared
      // project file.
      if (currentProjectPath() != g_sessionProjectPath) {
        leaveSession("project_changed", json::object(), "project changed, so the session ended");
        return;
      }
      // The owner stopping the session (farewell broadcast, or the
      // owner-vanished fallback) surfaces as status "ended": drop out
      // cleanly instead of editing into a dead doc. A deliberate stop sends
      // the user back to the lobby; a vanished host triggers an automatic
      // rejoin attempt first.
      if (g_mode == Mode::ClientActive) {
        json st = handleJson(reamesh_status, json::object());
        if (jsonStr(st, "state") == "ended") {
          onSessionEnded(st);
          return;
        }
        updateLoadingState(st);
      }
      unsigned peers = reamesh_peer_count(g_handle);
      if (peers != g_lastPeerCount) {
        if (peers > g_lastPeerCount)
          logEvent("peer_joined", json::object(), "a peer joined the session");
        else
          logEvent("peer_left", json::object(), "a peer left the session");
        g_lastPeerCount = peers;
      }
      pushLocalPresence();
      followTick();
      drainPings();
      if (g_mode == Mode::Hosting) refreshSessionMeta(false);
      // Capture before apply, so an edit made by the user in this tick
      // interval is diffed against the old baseline before remote
      // application re-baselines. A no-FX client NEVER captures: it is
      // view-only, and proposing anything from a stripped project would sooner
      // or later wipe the host's plugins.
      if (!g_noFx) captureLocalChanges(/*force=*/false);
      applyRemoteChanges();
      processDeferredItems();
      processDeferredRecv();
      processPeakJobs();
      // Last: re-paint the local lock tints. After capture (so the grey is
      // already invisible to it via the re-baseline) and after the applies (so a
      // remotely-recolored locked item re-tints with its fresh real color).
      reconcileItemTints();
      return;
    }
  }
}

// ---------- actions / entry point ----------

static bool dispatchCommand(int command) {
  if (command == 0) return false;
  // Save protection: just before REAPER serializes the project, drop the local
  // lock tints so the REAL item colors hit the .rpp, never our grey. We do NOT
  // consume the action (return false): REAPER still performs the save; the next
  // tick's reconcileItemTints re-applies whatever is still locked. Covers the
  // saves and the actions that end in a "save changes?" prompt; auto-save and
  // the like are covered by the tint ledger (see the note in the header).
  if (isSaveCommand(command)) restoreAllItemTints();
  if (command == g_cmdHost) { onHost(""); return true; }
  if (command == g_cmdLeave) { onLeave(); return true; }
  return false;
}

static bool hookCommand(int command, int flag) {
  (void)flag;
  return dispatchCommand(command);
}

static bool hookCommand2(KbdSectionInfo* sec, int command, int val, int valhw,
                         int relmode, HWND hwnd) {
  (void)sec; (void)val; (void)valhw; (void)relmode; (void)hwnd;
  return dispatchCommand(command);
}

extern "C" REAPER_PLUGIN_DLL_EXPORT int REAPER_PLUGIN_ENTRYPOINT(
    REAPER_PLUGIN_HINSTANCE hInstance, reaper_plugin_info_t* rec) {
  (void)hInstance;
  if (!rec) {
    // Plugin unloading.
    if (g_rec && g_unsupported) {
      // Stub mode registered the timer and nothing else, and there is no
      // session to leave (leaveSession would call into REAPER).
      g_rec->Register("-timer", (void*)timerTick);
      g_rec = nullptr;
      return 0;
    }
    if (g_rec) {
      g_rec->Register("-timer", (void*)timerTick);
      g_rec->Register("-hookcommand", (void*)hookCommand);
      g_rec->Register("-hookcommand2", (void*)hookCommand2);
      g_rec->Register("-atexit", (void*)restoreAllItemTints);
      g_rec->Register("-projectconfig", &g_tintLedgerReg);
      g_rec->Register("-custom_action", &g_caHost);
      g_rec->Register("-custom_action", &g_caLeave);
    }
    leaveSession("", json::object(), "");
    g_rec = nullptr;
    return 0;
  }
  if (rec->caller_version != REAPER_PLUGIN_VERSION || !rec->GetFunc) return 0;
  // A missing import used to abort the load, which left the user with an app
  // that calls the extension "not running" and no hint why. Load anyway and
  // let the app explain: too old a REAPER (by version, or by a missing
  // function) runs the stub mode, which touches no REAPER API past this point.
  const int missingApis = REAPERAPI_LoadAPI(rec->GetFunc);
  const std::string appVersion = GetAppVersion ? GetAppVersion() : "";
  g_reaperVersion = reaperVersionLabel(appVersion);
  if (missingApis != 0 || !reaperVersionSupported(appVersion)) {
    g_unsupported = true;
    g_rec = rec;
    rec->Register("timer", (void*)timerTick);
    reamesh_ctl_start();
    return 1;
  }

  g_rec = rec;
  // Lasting per-install network identity: persisted under REAPER's resource
  // dir, so this machine keeps one peer id across sessions and restarts
  // (remembered hosts, tickets that survive). Resettable from the app.
  {
    const std::string idPath = std::string(GetResourcePath()) + "/reamesh-identity.key";
    reamesh_identity_init(idPath.c_str());
  }
  g_cmdHost = rec->Register("custom_action", &g_caHost);
  g_cmdLeave = rec->Register("custom_action", &g_caLeave);
  rec->Register("hookcommand", (void*)hookCommand);
  rec->Register("hookcommand2", (void*)hookCommand2);
  rec->Register("timer", (void*)timerTick);
  // Restore real item colours when REAPER quits. Probably only after the quit
  // prompt (dispatchCommand covers that); kept as a last tidy-up.
  rec->Register("atexit", (void*)restoreAllItemTints);
  // Every project save carries the real colours of tinted items; every load
  // puts them back (see the persisted-tint ledger).
  rec->Register("projectconfig", &g_tintLedgerReg);
  reamesh_ctl_start();
  return 1;
}
