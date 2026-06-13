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
// Send status, errors, and activity to the companion app through the local
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
#define REAPERAPI_WANT_ReorderSelectedTracks
#define REAPERAPI_WANT_TrackList_AdjustWindows
#define REAPERAPI_WANT_UpdateArrange
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
// Live presence (playhead + selection) and arrange-view geometry for the
// peer-presence overlay. All long-stable APIs, so importing them is safe on
// older REAPER (REAPERAPI_LoadAPI only aborts on a missing wanted function).
#define REAPERAPI_WANT_CountSelectedTracks
#define REAPERAPI_WANT_CountSelectedMediaItems
#define REAPERAPI_WANT_GetSelectedMediaItem
#define REAPERAPI_WANT_GetMediaItemInfo_Value
#define REAPERAPI_WANT_GetPlayState
#define REAPERAPI_WANT_GetPlayPosition
#define REAPERAPI_WANT_GetCursorPosition
#define REAPERAPI_WANT_GetSet_ArrangeView2

#include "reaper_plugin.h"
#include "reaper_plugin_functions.h"

#include "reamesh_ffi.h"
#include "json.hpp"

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <filesystem>
#include <fstream>
#include <functional>
#include <iterator>
#include <map>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

using nlohmann::json;
namespace fs = std::filesystem;

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

enum class Mode { Idle, Hosting, Browsing, Downloading, ClientActive };

static reaper_plugin_info_t* g_rec = nullptr;
static ReameshHandle* g_handle = nullptr;
static Mode g_mode = Mode::Idle;
static std::string g_sessionProjectPath;  // path the session is bound to
static Snapshot g_snapshot;               // last known state, by GUID
static int g_lastChangeCount = -1;
static bool g_applyingRemote = false;  // echo-suppression / reentrancy guard
static unsigned g_lastPeerCount = 0;
// Last session-ending event, shown by the app as an error toast. Structured
// like a log entry ({code, params, text}); JSON null when there is none.
static json g_lastError;
static std::vector<DeferredItem> g_deferredItems;
static std::chrono::steady_clock::time_point g_lastCapture{};
static std::chrono::steady_clock::time_point g_lastFullScan{};
// Client only: cumulative count of own proposals the owner overrode, as
// reported by reamesh_status ("reverts"); deltas become activity-log events.
static long g_lastReverts = 0;

// In-flight automatic rejoin after the host vanished without a farewell
// (network blip / crash). While set, the Browsing tick watches the lobby for
// the same session id and rejoins with the remembered passphrase.
struct Reconnect {
  std::string sessionId;
  std::string sessionName;
  std::string passphrase;
  std::chrono::steady_clock::time_point deadline;
};
static std::optional<Reconnect> g_reconnect;
// Passphrase the current session was joined with, kept for auto-rejoin.
static std::string g_joinPassphrase;

// Two parameter-free convenience actions; the companion app is the real UI
// and drives everything (browse/join/leave) over the control server instead.
static int g_cmdHost = 0, g_cmdLeave = 0;
static custom_action_register_t g_caHost = {
    0, "REAMESH_HOST", "ReaMesh: Host session (share current project on LAN)", nullptr};
static custom_action_register_t g_caLeave = {
    0, "REAMESH_LEAVE", "ReaMesh: Leave session / stop hosting", nullptr};

// Activity log streamed to the companion app (never the REAPER console).
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
static bool isVolatileChunkLine(std::string_view trimmed) {
  return trimmed.rfind("SEL ", 0) == 0 || trimmed.rfind("SHOW ", 0) == 0 ||
         trimmed.rfind("LASTSEL ", 0) == 0 || trimmed.rfind("DOCKED ", 0) == 0 ||
         trimmed.rfind("FLOAT", 0) == 0 || trimmed.rfind("WNDRECT ", 0) == 0 ||
         trimmed.rfind("ARM ", 0) == 0 || trimmed.rfind("VIS ", 0) == 0;
}

static std::string normalizeChunk(const std::string& chunk) {
  std::string out;
  out.reserve(chunk.size());
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    std::string_view line(chunk.data() + pos, eol - pos);
    if (!isVolatileChunkLine(ltrim(line))) {
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
static std::vector<std::string> chunkFilePaths(const std::string& chunk) {
  std::vector<std::string> out;
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    std::string_view t = ltrim(std::string_view(chunk.data() + pos, eol - pos));
    if (t.rfind("FILE ", 0) == 0) {
      size_t q1 = t.find('"');
      size_t q2 = (q1 == std::string_view::npos) ? std::string_view::npos : t.find('"', q1 + 1);
      if (q2 != std::string_view::npos)
        out.emplace_back(t.substr(q1 + 1, q2 - q1 - 1));
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
  return fs::path(g_sessionProjectPath).parent_path();
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
    char buf[160];
    snprintf(buf, sizeof(buf), "%s\t%d\t%.17g\t%.17g\t%d\t", isrgn ? "R" : "M", num, pos, rgnend,
             color);
    out += buf;
    out += sanitizeMarkerName(name);
    out += '\n';
    idx = next;
  }
  return out;
}

static void applyMarkers(const std::string& s) {
  int nm = 0, nr = 0;
  const int total = CountProjectMarkers(nullptr, &nm, &nr);
  for (int i = total - 1; i >= 0; --i) DeleteProjectMarkerByIndex(nullptr, i);

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
    AddProjectMarker2(nullptr, isrgn, mpos, rgnend, name.c_str(), num, color);
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
  return buf.data();
}

static void applyProjectNotes(const std::string& notes) {
  std::vector<char> buf(notes.begin(), notes.end());
  buf.push_back('\0');
  GetSetProjectNotes(nullptr, true, buf.data(), 0);
}

// All 64 track-group names as one '\n'-joined field (names can't contain
// newlines; the Project Settings field is single-line).
static std::string captureGroupNames() {
  std::string out;
  for (int i = 1; i <= 64; ++i) {
    char desc[32];
    snprintf(desc, sizeof(desc), "TRACK_GROUP_NAME:%d", i);
    if (i > 1) out += '\n';
    out += captureProjectString(desc);
  }
  return out;
}

static void applyGroupNames(const std::string& s) {
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
  const int ni = CountMediaItems(nullptr);
  for (int i = 0; i < ni; ++i) {
    MediaItem* item = GetMediaItem(nullptr, i);
    ItemState is;
    is.track = trackGuid(GetMediaItem_Track(item));
    is.chunk = normalizeChunk(objectStateChunk(item));
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
static void internalizeExternalMedia() {
  const fs::path projDir = projectDir();
  if (projDir.empty()) return;
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
    fs::path source = fs::path(fn);
    fs::path rel = fs::relative(source, projDir, ec);
    if (!ec && !rel.empty() && rel.native().rfind("..", 0) != 0) continue;  // already inside

    fs::path dest = projDir / source.filename();
    for (int k = 2; fs::exists(dest, ec) && fs::file_size(dest, ec) != fs::file_size(source, ec);
         ++k)
      dest = projDir / (source.stem().string() + " (" + std::to_string(k) + ")" +
                        source.extension().string());
    if (!fs::exists(dest, ec)) {
      fs::copy_file(source, dest, ec);
      if (ec) {
        logEvent("media_copy_failed", {{"file", source.filename().string()}},
                 "could not copy " + source.filename().string() + " into the project: " +
                     ec.message());
        continue;
      }
    }
    PCM_source* ns = PCM_Source_CreateFromFile(dest.string().c_str());
    if (!ns) continue;
    SetMediaItemTake_Source(take, ns);
    PCM_Source_Destroy(src);
    UpdateItemInProject(item);
    logEvent("media_copied", {{"file", source.filename().string()}},
             "copied " + source.filename().string() + " into the project folder so peers get it");
  }
}

static void captureLocalChanges() {
  const int cc = GetProjectStateChangeCount(nullptr);
  const auto now = std::chrono::steady_clock::now();
  // Fast path: the project change counter moved (any undoable edit). Safety
  // net: some edits reach the chunks without promptly bumping the counter
  // (FX-parameter gestures get batched undo points, some toggles defer
  // theirs), so a full re-diff also runs unconditionally every 2s; the gate
  // is a latency optimization, never a correctness dependency.
  const bool counterMoved = cc != g_lastChangeCount;
  const bool rescanDue = now - g_lastFullScan >= std::chrono::seconds(2);
  if (!counterMoved && !rescanDue) return;
  // Chunk serialization isn't free; cap capture rate (sync target is ~1s).
  if (!rescanDue && now - g_lastCapture < std::chrono::milliseconds(250)) return;
  g_lastCapture = now;
  g_lastFullScan = now;
  g_lastChangeCount = cc;

  internalizeExternalMedia();
  Snapshot snap = snapshotProject();
  std::string changeset = diffSnapshots(g_snapshot, snap);
  g_snapshot = std::move(snap);
  if (changeset.empty()) return;  // change count moved for something we don't sync

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
    const std::string names = fields["lanenames"].get<std::string>();
    size_t pos = 0;
    for (int lane = 0; pos <= names.size(); ++lane) {
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
  for (int i = 0; i < (int)order.size(); ++i) {
    if (trackGuid(GetTrack(nullptr, i)) == order[i].guid) continue;
    MediaTrack* want = findTrackByGuid(order[i].guid);
    if (!want) continue;
    SetOnlyTrackSelected(want);
    ReorderSelectedTracks(i, 0);
  }
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
    if (!fs::exists(projDir / fs::path(p), ec)) missing.push_back(p);
  }
  return missing;
}

// Apply one item's fields (creation, move across tracks, content update,
// deletion). Caller has verified the referenced media exists.
static void applyItem(const std::string& guid, const json& fields) {
  MediaItem* item = findItemByGuid(guid);
  if (fields.value("present", true) == false) {
    if (item) DeleteTrackMediaItem(GetMediaItem_Track(item), item);
    return;
  }
  MediaTrack* tr = nullptr;
  if (fields.contains("track") && fields["track"].is_string())
    tr = findTrackByGuid(fields["track"].get<std::string>());
  if (!item) {
    if (!tr) return;  // no destination track (yet); the full-sync will retry
    item = AddMediaItemToTrack(tr);
    if (!item) return;
  } else if (tr && GetMediaItem_Track(item) != tr) {
    MoveMediaItemToTrack(item, tr);
  }
  if (fields.contains("chunk") && fields["chunk"].is_string()) {
    // The chunk carries the originator's item GUID, so identity is stamped
    // onto newly created items as a side effect.
    setObjectStateChunk(item, fields["chunk"].get<std::string>());
  }
  UpdateItemInProject(item);
}

static void applyChangeset(const json& changeset) {
  if (changeset.contains("tracks") && changeset["tracks"].is_object()) {
    const json& tracks = changeset["tracks"];

    // Deletions first so insertion indices below land in a settled list.
    for (auto it = tracks.begin(); it != tracks.end(); ++it) {
      const json& fields = it.value();
      if (fields.is_object() && fields.value("present", true) == false) {
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
      if (!fields.is_object() || fields.value("present", true) == false) continue;

      MediaTrack* tr = findTrackByGuid(guid);
      bool created = false;
      if (!tr) {
        int idx = (int)fields.value("index", (double)CountTracks(nullptr));
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
      applyTrackFields(tr, fields);
      if (created) {
        std::string nm = fields.value("name", std::string());
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
    // indices here. A receive whose source track is missing on this side is
    // dropped (it re-applies the next time the field changes or full state is
    // rebroadcast).
    for (const auto& [guid, wire] : pendingRecv) {
      MediaTrack* tr = findTrackByGuid(guid);
      if (!tr) continue;
      std::string local = transformRecvUnits(wire, [](const std::string& tok) -> std::string {
        MediaTrack* src = findTrackByGuid(tok);
        if (!src) return "";
        const int n = (int)GetMediaTrackInfo_Value(src, "IP_TRACKNUMBER");
        return n > 0 ? std::to_string(n - 1) : "";
      });
      std::string cur = objectStateChunk(tr);
      if (cur.empty()) continue;
      TrackBlocksPatch patch;
      patch.recv = &local;
      std::string next = replaceTrackBlocks(cur, patch);
      if (next != cur) setObjectStateChunk(tr, next);
    }
  }

  if (changeset.contains("items") && changeset["items"].is_object()) {
    const json& items = changeset["items"];
    for (auto it = items.begin(); it != items.end(); ++it) {
      const std::string& guid = it.key();
      const json& fields = it.value();
      if (!fields.is_object()) continue;

      std::vector<std::string> missing = missingMediaIn(fields);
      if (!missing.empty()) {
        // Media hasn't arrived yet: fetch it from the peers and apply the
        // item once it exists. Latest fields win for the same item.
        for (const std::string& rel : missing) reamesh_fetch_file(g_handle, rel.c_str());
        std::erase_if(g_deferredItems, [&](const DeferredItem& d) { return d.guid == guid; });
        g_deferredItems.push_back(
            {guid, fields, std::move(missing), std::chrono::steady_clock::now()});
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

static void applyRemoteChanges() {
  char* raw = reamesh_drain_remote(g_handle);
  std::string payload = raw ? raw : "[]";
  reamesh_free_string(raw);
  if (payload.empty() || payload == "[]") return;

  json changesets = json::parse(payload, nullptr, /*allow_exceptions=*/false);
  if (!changesets.is_array() || changesets.empty()) return;

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
  for (const auto& cs : changesets) applyChangeset(cs);
  PreventUIRefresh(-1);
  TrackList_AdjustWindows(false);
  UpdateArrange();

  g_snapshot = snapshotProject();
  g_lastChangeCount = GetProjectStateChangeCount(nullptr);
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
                  [&](const std::string& rel) { return fs::exists(projDir / fs::path(rel), ec); });
    if (d.missing.empty()) {
      ready.push_back(std::move(d));
      return true;
    }
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
  g_applyingRemote = true;
  PreventUIRefresh(1);
  for (const DeferredItem& d : ready) applyItem(d.guid, d.fields);
  PreventUIRefresh(-1);
  UpdateArrange();
  g_snapshot = snapshotProject();
  g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  g_applyingRemote = false;
  logEvent("media_applied", {{"count", ready.size()}},
           "applied " + std::to_string(ready.size()) + " item(s) after media arrived");
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
static json handleJson(char* (*fn)(ReameshHandle*), const json& fallback);

// Tear the session down. `code`/`params`/`text` describe why as a structured
// log event (also flagged to the app as the error); empty text = silent.
static void leaveSession(const char* code, json params, const std::string& text) {
  if (g_handle) {
    reamesh_shutdown(g_handle);
    g_handle = nullptr;
  }
  g_mode = Mode::Idle;
  g_sessionProjectPath.clear();
  g_snapshot = Snapshot{};
  g_deferredItems.clear();
  g_lastChangeCount = -1;
  g_lastPeerCount = 0;
  g_lastReverts = 0;
  if (!text.empty()) g_lastError = logEvent(code, std::move(params), text);
}

// Start LAN session discovery (Idle -> Browsing). Idempotent while browsing.
static bool startBrowse() {
  if (g_mode == Mode::Browsing) return true;
  if (g_mode != Mode::Idle) return false;
  std::string staging = std::string(GetResourcePath()) + "/ReaMesh";
  g_handle = reamesh_join(staging.c_str());
  if (!g_handle) {
    g_lastError = logEvent("browse_failed", json::object(), "could not start session discovery");
    return false;
  }
  g_mode = Mode::Browsing;
  g_lastError = json();
  logEvent("browsing", json::object(), "searching for sessions on the network");
  return true;
}

// Join one of the announced sessions (Browsing -> Downloading).
static bool joinSessionById(const std::string& sid, const std::string& sname,
                            const std::string& passphrase) {
  if (g_mode != Mode::Browsing || !g_handle) return false;
  if (!reamesh_join_session(g_handle, sid.c_str(), passphrase.c_str())) {
    logEvent("join_unavailable", {{"name", sname}},
             "could not join \"" + sname + "\" (it may have just closed); try again");
    return false;
  }
  // Rust resets its cumulative revert counter for the new session; match it
  // here so the first real override is detected as a delta rather than masked
  // by a count left over from a previous session (e.g. across an auto-rejoin).
  g_lastReverts = 0;
  g_mode = Mode::Downloading;
  g_lastError = json();
  g_joinPassphrase = passphrase;
  logEvent("joining", {{"name", sname}}, "joining \"" + sname + "\", downloading the project");
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
  rc.deadline = std::chrono::steady_clock::now() + std::chrono::seconds(45);
  leaveSession("", json::object(), "");  // silent: this is not (yet) an error
  if (rc.sessionId.empty() || !startBrowse()) {
    g_lastError =
        logEvent("host_lost", json::object(), "lost the connection to the host");
    return;
  }
  logEvent("reconnecting", {{"name", rc.sessionName}},
           "lost the host, trying to reconnect to \"" + rc.sessionName + "\"");
  g_reconnect = std::move(rc);
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
  json sessions = handleJson(reamesh_sessions, json::array());
  if (!sessions.is_array()) return;
  for (const auto& s : sessions) {
    if (s.is_object() && jsonStr(s, "session_id") == g_reconnect->sessionId) {
      Reconnect rc = *g_reconnect;
      g_reconnect.reset();
      joinSessionById(rc.sessionId, rc.sessionName, rc.passphrase);
      return;
    }
  }
}

static void onHost(const std::string& passphrase) {
  if (g_mode == Mode::Hosting) return;
  if (g_mode != Mode::Idle) leaveSession("left", json::object(), "left previous session");

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
  muteActivity(std::chrono::seconds(4));
  g_lastError = json();
  g_sessionProjectPath = path;
  g_snapshot = Snapshot{};
  g_lastChangeCount = -1;  // force a full capture; this seeds the shared doc
  g_lastCapture = {};
  g_lastFullScan = {};
  g_lastPeerCount = 0;
  logEvent("hosting", {{"name", name}},
           passphrase.empty()
               ? "hosting \"" + name + "\", sharing all files under " + dir
               : "hosting \"" + name + "\" (passphrase-protected), sharing " + dir);
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

// ---------- companion app control server ----------
//
// The Rust lib runs a localhost WebSocket server (reamesh_ctl_*) that the
// ReaMesh companion app connects to. The app's commands are queued there and
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
  double play = 0.0;
  bool havePlay = false;
  for (const auto& p : peers) {
    if (p.is_object() && jsonStr(p, "id") == peerId) {
      guid = jsonStr(p, "focus");
      if (p.contains("play") && p["play"].is_number()) {
        play = p["play"].get<double>();
        havePlay = true;
      }
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
  // Horizontal: scroll the arrange so their playhead sits in the middle,
  // keeping our own zoom and without moving our edit cursor. This is what makes
  // "jump to their view" actually land somewhere when their track was already
  // on screen but they're working elsewhere in the timeline.
  if (havePlay) {
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

static void ctlDispatch(const json& cmd) {
  const std::string c = jsonStr(cmd, "cmd");
  // Any explicit user action supersedes a pending auto-rejoin.
  if (c == "host" || c == "join" || c == "leave") g_reconnect.reset();
  if (c == "browse") {
    startBrowse();
  } else if (c == "host") {
    onHost(jsonStr(cmd, "passphrase"));
  } else if (c == "join") {
    if (g_mode == Mode::Idle) startBrowse();
    std::string name = jsonStr(cmd, "session_name");
    joinSessionById(jsonStr(cmd, "session_id"), name.empty() ? "session" : name,
                    jsonStr(cmd, "passphrase"));
  } else if (c == "leave") {
    onLeave();
  } else if (c == "reveal_peer") {
    revealPeer(jsonStr(cmd, "peer_id"));
  }
}

// Geometry for the arrange overlay (peer playheads + selection ghosts), in
// REAPER SCREEN coordinates (logical points, top-left origin). The overlay
// window covers REAPER's whole main window, so the webview converts to
// window-relative by subtracting its own screen origin, which is why we never
// need the arrange VIEWPORT rect (REAPER doesn't expose it through the stable
// API). Per-track `P_UI_RECT:tcp.size` already reflects vertical scroll, and the
// time axis comes from GetSet_ArrangeView2; together they're enough.
//
// The exact P_UI_RECT string format and screen-coordinate space want a one-time
// on-device check, so the first raw rect is logged to stderr. Returns {} when
// there's nothing to draw (not in a session, or no resolvable geometry).
static json computeArrangeView(const json& peers) {
  if (g_mode != Mode::Hosting && g_mode != Mode::ClientActive) return json::object();

  // Time axis: t0 = project time at the arrange's left edge; spp = seconds per
  // horizontal pixel (from the time at two x offsets, 0 and 1000).
  double t0 = 0, t1 = 0, a = 0, b = 0;
  GetSet_ArrangeView2(nullptr, false, 0, 0, &t0, &t1);
  GetSet_ArrangeView2(nullptr, false, 0, 1000, &a, &b);
  const double spp = (b - a) / 1000.0;
  if (!(spp > 0.0)) return json::object();

  // Per-track screen rects. P_UI_RECT reflects the live vertical scroll, so a
  // track's y is correct right now; the arrange's left x is the right edge of
  // the (uniform-width) TCP.
  json tracks = json::object();
  double ax = 0;
  bool haveAx = false;
  double top = 0, bottom = 0;
  bool haveSpan = false;
  static bool loggedRect = false;
  const int n = CountTracks(nullptr);
  for (int i = 0; i < n; ++i) {
    MediaTrack* tr = GetTrack(nullptr, i);
    if (!tr) continue;
    char buf[128] = {0};
    if (!GetSetMediaTrackInfo_String(tr, "P_UI_RECT:tcp.size", buf, false)) continue;
    if (!loggedRect) {
      fprintf(stderr, "[reamesh] P_UI_RECT:tcp.size = \"%s\"\n", buf);
      loggedRect = true;
    }
    double rx = 0, ry = 0, rw = 0, rh = 0;
    if (sscanf(buf, "%lf %lf %lf %lf", &rx, &ry, &rw, &rh) != 4) continue;
    if (rh <= 0) continue;  // collapsed / hidden
    if (!haveAx) {
      ax = rx + rw;
      haveAx = true;
    }
    if (!haveSpan) {
      top = ry;
      bottom = ry + rh;
      haveSpan = true;
    } else {
      if (ry < top) top = ry;
      if (ry + rh > bottom) bottom = ry + rh;
    }
    tracks[trackGuid(tr)] = {{"y", ry}, {"h", rh}};
  }
  if (!haveAx) return json::object();

  // Items any peer has selected -> their owning track + position/length, so the
  // overlay can outline what each person is "holding".
  json items = json::object();
  if (peers.is_array()) {
    for (const auto& p : peers) {
      if (!p.is_object() || !p.contains("seli") || !p["seli"].is_array()) continue;
      for (const auto& g : p["seli"]) {
        if (!g.is_string()) continue;
        const std::string ig = g.get<std::string>();
        if (items.contains(ig)) continue;
        MediaItem* it = findItemByGuid(ig);
        if (!it) continue;
        MediaTrack* itr = GetMediaItem_Track(it);
        items[ig] = {{"tr", itr ? trackGuid(itr) : std::string()},
                     {"pos", GetMediaItemInfo_Value(it, "D_POSITION")},
                     {"len", GetMediaItemInfo_Value(it, "D_LENGTH")}};
      }
    }
  }

  return json{{"ax", ax},   {"t0", t0},         {"spp", spp},
              {"top", top}, {"bottom", bottom}, {"tracks", std::move(tracks)},
              {"items", std::move(items)}};
}

static void ctlTick() {
  for (;;) {
    char* raw = reamesh_ctl_poll();
    if (!raw) break;
    json cmd = json::parse(raw, nullptr, false);
    reamesh_free_string(raw);
    if (cmd.is_object()) ctlDispatch(cmd);
  }

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
      if (guid.empty()) continue;
      if (MediaTrack* tr = findTrackByGuid(guid)) {
        int n = static_cast<int>(GetMediaTrackInfo_Value(tr, "IP_TRACKNUMBER"));
        if (n > 0) p["track"] = n;
      }
    }
  }
  json st = handleJson(reamesh_status, json::object());
  // Client: the owner overrode some of this peer's proposals since the last
  // tick: log it (before the log is mirrored below) and surface a cumulative
  // counter so the app can toast on the increase.
  long reverts = (st.contains("reverts") && st["reverts"].is_number_integer())
                     ? st["reverts"].get<long>()
                     : 0;
  if (g_mode == Mode::ClientActive) {
    if (reverts > g_lastReverts) {
      const long delta = reverts - g_lastReverts;
      // The status carries a tail of recently-overridden (section,id) pairs; the
      // last `delta` are new. Name the tracks among them so the app can say
      // *what* the host overrode, not just that something snapped back.
      std::vector<std::string> names;
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
          std::string nm = trackNameByGuid(id);
          if (!nm.empty()) names.push_back(nm);
        }
      }
      // With active-editor-wins these only fire on a genuine simultaneous
      // conflict, so they're rare, but route them through activityEvent so a
      // burst on the same track still collapses to one notice (de-dupes
      // identical text within a few seconds) instead of spamming the feed.
      if (names.empty()) {
        activityEvent("edit_overridden", {{"count", delta}}, "the host overrode your change");
      } else {
        for (const std::string& nm : names)
          activityEvent("edit_overridden", {{"track", nm}},
                        "the host overrode your change to " + nm);
      }
      g_lastReverts = reverts;
    }
  } else {
    reverts = 0;
  }
  state["type"] = "state";
  state["mode"] = modeName();
  state["project"] = fileStem(currentProjectPath());
  state["error"] = g_lastError;
  state["log"] = g_log;
  state["overridden"] = reverts;
  state["reconnecting"] = static_cast<bool>(g_reconnect);
  state["state"] = jsonStr(st, "state");
  state["progress"] =
      (st.contains("progress") && st["progress"].is_number()) ? st["progress"].get<double>() : 0.0;
  std::string session = jsonStr(st, "session_name");
  if (session.empty() && !g_sessionProjectPath.empty()) session = fileStem(g_sessionProjectPath);
  state["session"] = session;
  state["sessions"] = handleJson(reamesh_sessions, json::array());
  state["view"] = computeArrangeView(peers);
  state["peers"] = std::move(peers);

  static std::string lastPublished;
  std::string out = state.dump();
  if (out != lastPublished) {
    lastPublished = out;
    reamesh_ctl_publish(out.c_str());
  }
}

// ---------- timer ----------

// Report the user's live presence so peers can see where each person is: the
// focused track, the playhead (the edit cursor, or the play position while the
// transport is rolling), and the selected tracks/items. GUIDs are the shared
// identity; each peer resolves them to its own project. Cheap value reads on
// the main-thread tick, pushed only when something changes; while playing,
// `play` advances every tick, which is exactly what keeps a sweeping playhead
// live on peers' overlays.
static void pushLocalPresence() {
  MediaTrack* focusTr = GetSelectedTrack(nullptr, 0);
  std::string focus = focusTr ? trackGuid(focusTr) : std::string();

  const int ps = GetPlayState();  // bit 0: playing, bit 2: recording
  const bool playing = (ps & 1) != 0;
  const double play = playing ? GetPlayPosition() : GetCursorPosition();

  json seltr = json::array();
  for (int i = 0, n = CountSelectedTracks(nullptr); i < n && (int)seltr.size() < 128; ++i)
    if (MediaTrack* tr = GetSelectedTrack(nullptr, i)) seltr.push_back(trackGuid(tr));

  json seli = json::array();
  for (int i = 0, n = CountSelectedMediaItems(nullptr); i < n && (int)seli.size() < 128; ++i)
    if (MediaItem* it = GetSelectedMediaItem(nullptr, i)) seli.push_back(itemGuid(it));

  json p = {{"focus", focus},   {"play", play},          {"playing", playing},
            {"seltr", std::move(seltr)}, {"seli", std::move(seli)}};
  static std::string last;
  std::string s = p.dump();
  if (s != last) {
    last = s;
    reamesh_set_presence(s.c_str());
  }
}

static void pollDownload() {
  char* raw = reamesh_status(g_handle);
  std::string payload = raw ? raw : "{}";
  reamesh_free_string(raw);
  json st = json::parse(payload, nullptr, false);
  if (!st.is_object()) return;
  std::string state = st.value("state", "");

  if (state == "ready") {
    std::string path = st.value("project_path", "");
    logEvent("download_complete", json::object(), "download complete, opening " + path);
    g_applyingRemote = true;
    Main_openProject(path.c_str());
    // The loaded project IS the owner's state; baseline it so the load is
    // never diffed as a burst of local edits.
    g_snapshot = snapshotProject();
    g_lastChangeCount = GetProjectStateChangeCount(nullptr);
    g_applyingRemote = false;
    g_mode = Mode::ClientActive;
    // The owner's full state lands as one big changeset right after this;
    // don't narrate every track of it as an "added" event.
    muteActivity(std::chrono::seconds(4));
    g_sessionProjectPath = currentProjectPath();
    logEvent("connected", json::object(),
             "connected: your edits are proposals; the session owner wins conflicts");
  } else if (state == "failed") {
    std::string msg = st.value("message", std::string("unknown error"));
    // The wrong-passphrase refusal is the one failure a user routinely hits;
    // give it a stable code so the app can translate it.
    if (msg.find("passphrase") != std::string::npos)
      leaveSession("wrong_password", json::object(), "join failed: " + msg);
    else
      leaveSession("join_failed", {{"reason", msg}}, "join failed: " + msg);
  } else if (state == "ended") {
    // The host went away while we were still downloading. Deliberate stop →
    // leave; vanished without a farewell → try to rejoin automatically (the
    // partial download dedups, so the retry is cheap).
    if (jsonStr(st, "end_reason") == "host_lost") beginReconnect(st);
    else leaveSession("host_ended", json::object(), "the host ended the session");
  }
}

static void timerTick() {
  if (g_applyingRemote) return;
  // Companion-app commands and state mirroring run in every mode, including
  // Idle (the app may connect before any session exists).
  ctlTick();
  if (!g_handle) return;

  switch (g_mode) {
    case Mode::Idle:
      return;
    case Mode::Browsing:
      reconnectTick();
      return;
    case Mode::Downloading:
      pollDownload();
      return;
    case Mode::Hosting:
    case Mode::ClientActive: {
      // Switching projects ends the session: its doc is bound to the shared
      // project file.
      if (currentProjectPath() != g_sessionProjectPath) {
        leaveSession("project_changed", json::object(), "project changed; session ended");
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
          if (jsonStr(st, "end_reason") == "host_lost") beginReconnect(st);
          else leaveSession("host_ended", json::object(), "the host ended the session");
          return;
        }
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
      // Capture before apply, so an edit made by the user in this tick
      // interval is diffed against the old baseline before remote
      // application re-baselines.
      captureLocalChanges();
      applyRemoteChanges();
      processDeferredItems();
      return;
    }
  }
}

// ---------- actions / entry point ----------

static bool dispatchCommand(int command) {
  if (command == 0) return false;
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
    if (g_rec) {
      g_rec->Register("-timer", (void*)timerTick);
      g_rec->Register("-hookcommand", (void*)hookCommand);
      g_rec->Register("-hookcommand2", (void*)hookCommand2);
      g_rec->Register("-custom_action", &g_caHost);
      g_rec->Register("-custom_action", &g_caLeave);
    }
    leaveSession("", json::object(), "");
    g_rec = nullptr;
    return 0;
  }
  if (rec->caller_version != REAPER_PLUGIN_VERSION || !rec->GetFunc) return 0;
  if (REAPERAPI_LoadAPI(rec->GetFunc) != 0) return 0;

  g_rec = rec;
  g_cmdHost = rec->Register("custom_action", &g_caHost);
  g_cmdLeave = rec->Register("custom_action", &g_caLeave);
  rec->Register("hookcommand", (void*)hookCommand);
  rec->Register("hookcommand2", (void*)hookCommand2);
  rec->Register("timer", (void*)timerTick);
  reamesh_ctl_start();
  return 1;
}
