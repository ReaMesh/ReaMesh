// ReaMesh REAPER extension: thin, synchronous capture/apply layer.
//
// Everything in this file runs on REAPER's main thread, driven by the
// registered timer and action hooks. The Rust library (reamesh_ffi.h) owns
// all networking, the CRDT doc and project-file transfers on its own
// background threads; we only exchange JSON strings with it and never block
// on the network.
//
// v3 sync model (owner-authoritative):
//   tracks:  index/name/vol/pan/mute/solo as scalar fields, plus the track's
//            FX chain (<FXCHAIN> block) and automation envelopes (top-level
//            *ENV* blocks) as normalized state-chunk strings.
//   items:   each media item as its full normalized state chunk (covers
//            position, length, splits, fades, takes, take FX, MIDI content)
//            plus the GUID of the track it sits on.
// Media files referenced by an incoming item that are missing locally are
// fetched from the session peers in the background; the item is applied as
// soon as its files exist (deferred apply).
//
// The extension never writes to the REAPER console: status, errors and an
// activity log are exposed to the ReaImGui panel through the ReaMesh_* API.

#define REAPERAPI_IMPLEMENT
#define REAPERAPI_MINIMAL
#define REAPERAPI_WANT_GetProjectStateChangeCount
#define REAPERAPI_WANT_CountTracks
#define REAPERAPI_WANT_GetTrack
#define REAPERAPI_WANT_GetTrackGUID
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
#define REAPERAPI_WANT_EnumProjects
#define REAPERAPI_WANT_Main_SaveProject
#define REAPERAPI_WANT_Main_openProject
#define REAPERAPI_WANT_GetResourcePath
#define REAPERAPI_WANT_AddRemoveReaScript
#define REAPERAPI_WANT_Main_OnCommand
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
#define REAPERAPI_WANT_Undo_BeginBlock2
#define REAPERAPI_WANT_Undo_EndBlock2
#define REAPERAPI_WANT_CountProjectMarkers
#define REAPERAPI_WANT_EnumProjectMarkers3
#define REAPERAPI_WANT_AddProjectMarker2
#define REAPERAPI_WANT_DeleteProjectMarkerByIndex
#define REAPERAPI_WANT_CountTempoTimeSigMarkers
#define REAPERAPI_WANT_GetTempoTimeSigMarker
#define REAPERAPI_WANT_SetTempoTimeSigMarker
#define REAPERAPI_WANT_DeleteTempoTimeSigMarker

#include "reaper_plugin.h"
#include "reaper_plugin_functions.h"

#include "reamesh_ffi.h"
#include "reamesh_gui_script.h"
#include "json.hpp"

#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <map>
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
  bool mute = false;
  bool solo = false;
  std::string fxchain;  // normalized <FXCHAIN> block, "" when no FX
  std::string env;      // concatenated normalized top-level *ENV* blocks
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
static std::string g_lastError;  // last session-ending message, shown by the GUI
static int g_guiScriptCmd = 0;   // command id of the installed GUI ReaScript
static std::vector<DeferredItem> g_deferredItems;
static std::chrono::steady_clock::time_point g_lastCapture{};

static int g_cmdHost = 0, g_cmdJoin = 0, g_cmdLeave = 0, g_cmdPanel = 0;
static custom_action_register_t g_caHost = {
    0, "REAMESH_HOST", "ReaMesh: Host session (share current project on LAN)", nullptr};
static custom_action_register_t g_caJoin = {
    0, "REAMESH_JOIN", "ReaMesh: Join session (opens the panel)", nullptr};
static custom_action_register_t g_caLeave = {
    0, "REAMESH_LEAVE", "ReaMesh: Leave session / stop hosting", nullptr};
static custom_action_register_t g_caPanel = {
    0, "REAMESH_PANEL", "ReaMesh: Open panel (GUI)", nullptr};

// Activity log shown by the GUI panel (never the REAPER console).
static std::deque<std::string> g_log;

static void logLine(const std::string& s) {
  g_log.push_back(s);
  while (g_log.size() > 80) g_log.pop_front();
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
// they bump the diff on every click.
static bool isVolatileChunkLine(std::string_view trimmed) {
  return trimmed.rfind("SEL ", 0) == 0 || trimmed.rfind("SHOW ", 0) == 0 ||
         trimmed.rfind("LASTSEL ", 0) == 0 || trimmed.rfind("DOCKED ", 0) == 0 ||
         trimmed.rfind("FLOAT", 0) == 0 || trimmed.rfind("WNDRECT ", 0) == 0;
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

// The synced top-level child blocks of a track chunk: the FX chain and the
// automation envelopes. Items (<ITEM>) deliberately stay out; they are
// synced per-item through the "items" section.
static bool isEnvTag(std::string_view tag) {
  return tag.find("ENV") != std::string_view::npos;
}

struct TrackBlocks {
  std::string fxchain;
  std::string env;
};

// Extract the FXCHAIN and envelope blocks from a (normalized) track chunk.
static TrackBlocks extractTrackBlocks(const std::string& chunk) {
  TrackBlocks out;
  int depth = 0;
  bool collecting = false;
  std::string cur, curTag;
  size_t pos = 0;
  while (pos < chunk.size()) {
    size_t eol = chunk.find('\n', pos);
    if (eol == std::string::npos) eol = chunk.size();
    std::string_view line(chunk.data() + pos, eol - pos);
    std::string_view t = ltrim(line);
    if (!collecting && depth == 1 && !t.empty() && t.front() == '<') {
      std::string_view tag = t.substr(1);
      tag = tag.substr(0, tag.find_first_of(" \t"));
      if (tag == "FXCHAIN" || isEnvTag(tag)) {
        collecting = true;
        curTag = std::string(tag);
        cur.clear();
      }
    }
    if (collecting) {
      cur.append(line);
      cur.push_back('\n');
    }
    if (!t.empty() && t.front() == '<') {
      ++depth;
    } else if (t == ">") {
      --depth;
      if (collecting && depth == 1) {
        if (curTag == "FXCHAIN") {
          out.fxchain = cur;
        } else {
          out.env += cur;
        }
        collecting = false;
      }
    }
    pos = eol + 1;
  }
  return out;
}

// Rebuild a track chunk with its FXCHAIN block replaced by `fxchain` and/or
// its envelope blocks replaced by `env` (nullptr = leave that kind alone,
// empty string = remove). Replacements land where the old blocks were, else
// before the first item, else before the closing ">".
static std::string replaceTrackBlocks(const std::string& chunk, const std::string* fxchain,
                                      const std::string* env) {
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
  size_t insertAt = std::string::npos;
  size_t firstItemAt = std::string::npos;

  for (const std::string& line : lines) {
    std::string_view t = ltrim(line);
    bool opens = !t.empty() && t.front() == '<';
    bool closes = (t == ">");

    if (skipUntilDepth < 0 && depth == 1 && opens) {
      std::string_view tag = t.substr(1);
      tag = tag.substr(0, tag.find_first_of(" \t"));
      bool removeFx = fxchain && tag == "FXCHAIN";
      bool removeEnv = env && isEnvTag(tag);
      if (removeFx || removeEnv) {
        if (insertAt == std::string::npos) insertAt = out.size();
        skipUntilDepth = depth;  // skip until we return to this depth
        ++depth;
        continue;
      }
      if (tag == "ITEM" && firstItemAt == std::string::npos) firstItemAt = out.size();
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

  if (insertAt == std::string::npos)
    insertAt = (firstItemAt != std::string::npos) ? firstItemAt
                                                  : (out.empty() ? 0 : out.size() - 1);

  std::string result;
  result.reserve(chunk.size() + (fxchain ? fxchain->size() : 0) + (env ? env->size() : 0));
  auto emit = [&](const std::string& l) {
    result += l;
    result += '\n';
  };
  for (size_t i = 0; i < out.size(); ++i) {
    if (i == insertAt) {
      if (fxchain && !fxchain->empty()) result += *fxchain;  // blocks end in '\n'
      if (env && !env->empty()) result += *env;
    }
    emit(out[i]);
  }
  if (insertAt >= out.size()) {
    if (fxchain && !fxchain->empty()) result += *fxchain;
    if (env && !env->empty()) result += *env;
  }
  return result;
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

static Snapshot snapshotProject() {
  Snapshot snap;
  snap.project.tempo = captureTempo();
  snap.project.markers = captureMarkers();
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
    ts.mute = GetMediaTrackInfo_Value(tr, "B_MUTE") != 0.0;
    ts.solo = GetMediaTrackInfo_Value(tr, "I_SOLO") != 0.0;
    TrackBlocks blocks = extractTrackBlocks(normalizeChunk(objectStateChunk(tr)));
    ts.fxchain = std::move(blocks.fxchain);
    ts.env = std::move(blocks.env);
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
  return json{{"present", true}, {"index", ts.index},     {"name", ts.name},
              {"vol", ts.vol},   {"pan", ts.pan},         {"mute", ts.mute},
              {"solo", ts.solo}, {"fxchain", ts.fxchain}, {"env", ts.env}};
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
    if (old.mute != ts.mute) delta["mute"] = ts.mute;
    if (old.solo != ts.solo) delta["solo"] = ts.solo;
    if (old.fxchain != ts.fxchain) delta["fxchain"] = ts.fxchain;
    if (old.env != ts.env) delta["env"] = ts.env;
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
        logLine("could not copy " + source.filename().string() + " into the project: " +
                ec.message());
        continue;
      }
    }
    PCM_source* ns = PCM_Source_CreateFromFile(dest.string().c_str());
    if (!ns) continue;
    SetMediaItemTake_Source(take, ns);
    PCM_Source_Destroy(src);
    UpdateItemInProject(item);
    logLine("copied " + source.filename().string() + " into the project folder so peers get it");
  }
}

static void captureLocalChanges() {
  const int cc = GetProjectStateChangeCount(nullptr);
  if (cc == g_lastChangeCount) return;
  // Chunk serialization isn't free; cap capture rate (sync target is ~1s).
  const auto now = std::chrono::steady_clock::now();
  if (now - g_lastCapture < std::chrono::milliseconds(250)) return;
  g_lastCapture = now;
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
  if (fields.contains("vol") && fields["vol"].is_number())
    SetMediaTrackInfo_Value(tr, "D_VOL", fields["vol"].get<double>());
  if (fields.contains("pan") && fields["pan"].is_number())
    SetMediaTrackInfo_Value(tr, "D_PAN", fields["pan"].get<double>());
  if (fields.contains("mute") && fields["mute"].is_boolean())
    SetMediaTrackInfo_Value(tr, "B_MUTE", fields["mute"].get<bool>() ? 1.0 : 0.0);
  if (fields.contains("solo") && fields["solo"].is_boolean())
    SetMediaTrackInfo_Value(tr, "I_SOLO", fields["solo"].get<bool>() ? 1.0 : 0.0);

  // FX chain / envelopes arrive as normalized chunk blocks; splice them into
  // the track's current chunk (after the scalar setters above, so their
  // values survive the chunk round-trip).
  const bool hasFx = fields.contains("fxchain") && fields["fxchain"].is_string();
  const bool hasEnv = fields.contains("env") && fields["env"].is_string();
  if (hasFx || hasEnv) {
    std::string fx = hasFx ? fields["fxchain"].get<std::string>() : "";
    std::string env = hasEnv ? fields["env"].get<std::string>() : "";
    std::string cur = objectStateChunk(tr);
    if (!cur.empty()) {
      std::string next =
          replaceTrackBlocks(cur, hasFx ? &fx : nullptr, hasEnv ? &env : nullptr);
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
        if (MediaTrack* tr = findTrackByGuid(it.key())) DeleteTrack(tr);
      }
    }

    std::map<std::string, double> desiredIndex;
    for (auto it = tracks.begin(); it != tracks.end(); ++it) {
      const std::string& guid = it.key();
      const json& fields = it.value();
      if (!fields.is_object() || fields.value("present", true) == false) continue;

      MediaTrack* tr = findTrackByGuid(guid);
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
      }
      applyTrackFields(tr, fields);
      if (fields.contains("index") && fields["index"].is_number())
        desiredIndex[guid] = fields["index"].get<double>();
    }
    if (!desiredIndex.empty()) applyReorder(desiredIndex);
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
        logLine("waiting for media of an incoming item...");
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
      if (f.contains("tempo") && f["tempo"].is_string())
        applyTempo(f["tempo"].get<std::string>());
      if (f.contains("markers") && f["markers"].is_string())
        applyMarkers(f["markers"].get<std::string>());
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
  // Wrap in an undo block so a remote edit is a single Ctrl+Z step. Undoing it
  // is captured as a normal local change next tick (and, on a client, proposed
  // back); there is no re-application loop because we re-baseline below.
  Undo_BeginBlock2(nullptr);
  PreventUIRefresh(1);
  for (const auto& cs : changesets) applyChangeset(cs);
  PreventUIRefresh(-1);
  Undo_EndBlock2(nullptr, "ReaMesh: apply remote changes", -1);
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
      logLine("gave up waiting for media: " + d.missing.front());
      return true;
    }
    // Keep nudging the fetch; it is idempotent while the file is missing.
    for (const std::string& rel : d.missing) reamesh_fetch_file(g_handle, rel.c_str());
    return false;
  });

  if (ready.empty()) return;
  g_applyingRemote = true;
  Undo_BeginBlock2(nullptr);
  PreventUIRefresh(1);
  for (const DeferredItem& d : ready) applyItem(d.guid, d.fields);
  PreventUIRefresh(-1);
  Undo_EndBlock2(nullptr, "ReaMesh: apply deferred media items", -1);
  UpdateArrange();
  g_snapshot = snapshotProject();
  g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  g_applyingRemote = false;
  logLine("applied " + std::to_string(ready.size()) + " item(s) after media arrived");
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

static void leaveSession(const std::string& why) {
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
  if (!why.empty()) {
    logLine(why);
    g_lastError = why;
  }
}

// Start LAN session discovery (Idle -> Browsing). Idempotent while browsing.
static bool startBrowse() {
  if (g_mode == Mode::Browsing) return true;
  if (g_mode != Mode::Idle) return false;
  std::string staging = std::string(GetResourcePath()) + "/ReaMesh";
  g_handle = reamesh_join(staging.c_str());
  if (!g_handle) {
    g_lastError = "could not start session discovery";
    logLine(g_lastError);
    return false;
  }
  g_mode = Mode::Browsing;
  g_lastError.clear();
  logLine("searching for sessions on the network");
  return true;
}

// Join one of the announced sessions (Browsing -> Downloading).
static bool joinSessionById(const std::string& sid, const std::string& sname,
                            const std::string& passphrase) {
  if (g_mode != Mode::Browsing || !g_handle) return false;
  if (!reamesh_join_session(g_handle, sid.c_str(), passphrase.c_str())) {
    logLine("could not join \"" + sname + "\" (it may have just closed); try again");
    return false;
  }
  g_mode = Mode::Downloading;
  g_lastError.clear();
  logLine("joining \"" + sname + "\", downloading the project");
  return true;
}

static void onHost(const std::string& passphrase) {
  if (g_mode == Mode::Hosting) return;
  if (g_mode != Mode::Idle) leaveSession("left previous session");

  std::string path = currentProjectPath();
  if (path.empty()) {
    // Unsaved project: trigger Save As so there is a project file to share.
    g_lastError = "save the project first, then host again";
    logLine(g_lastError);
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
    g_lastError = "could not start hosting";
    logLine(g_lastError);
    return;
  }
  g_mode = Mode::Hosting;
  g_lastError.clear();
  g_sessionProjectPath = path;
  g_snapshot = Snapshot{};
  g_lastChangeCount = -1;  // force a full capture; this seeds the shared doc
  g_lastCapture = {};
  g_lastPeerCount = 0;
  logLine(passphrase.empty()
              ? "hosting \"" + name + "\", sharing all files under " + dir
              : "hosting \"" + name + "\" (passphrase-protected), sharing " + dir);
}

static void onLeave() {
  if (g_mode == Mode::Idle) return;
  leaveSession("left the session");
  g_lastError.clear();  // user-initiated, nothing to flag in the GUI
}

static void openPanel() {
  if (g_guiScriptCmd) Main_OnCommand(g_guiScriptCmd, 0);
}

// ---------- ReaScript API (drives the ReaImGui panel) ----------

static std::string jsonStr(const json& j, const char* key) {
  auto it = j.find(key);
  return (it != j.end() && it->is_string()) ? it->get<std::string>() : "";
}

// Current state as key=value lines (trivial to parse from Lua, no JSON
// library needed there).
static const char* ReaMesh_GetStatus() {
  static std::string buf;
  const char* mode = "idle";
  switch (g_mode) {
    case Mode::Hosting: mode = "hosting"; break;
    case Mode::Browsing: mode = "browsing"; break;
    case Mode::Downloading: mode = "downloading"; break;
    case Mode::ClientActive: mode = "client"; break;
    case Mode::Idle: break;
  }
  std::string state, session, path;
  double progress = 0.0;
  unsigned peers = 0;
  if (g_handle) {
    char* raw = reamesh_status(g_handle);
    json st = json::parse(raw ? raw : "{}", nullptr, false);
    reamesh_free_string(raw);
    if (st.is_object()) {
      state = jsonStr(st, "state");
      session = jsonStr(st, "session_name");
      path = jsonStr(st, "project_path");
      if (st.contains("progress") && st["progress"].is_number())
        progress = st["progress"].get<double>();
    }
    peers = reamesh_peer_count(g_handle);
  }
  if (session.empty() && !g_sessionProjectPath.empty())
    session = fileStem(g_sessionProjectPath);
  buf = "mode=" + std::string(mode) + "\nstate=" + state + "\nsession=" + session +
        "\nprogress=" + std::to_string(progress) + "\npeers=" + std::to_string(peers) +
        "\npath=" + path + "\nerror=" + g_lastError;
  return buf.c_str();
}

// Announced sessions, one per line: session_id \t session_name \t project_file
static const char* ReaMesh_GetSessions() {
  static std::string buf;
  buf.clear();
  if (g_handle) {
    char* raw = reamesh_sessions(g_handle);
    json sessions = json::parse(raw ? raw : "[]", nullptr, false);
    reamesh_free_string(raw);
    if (sessions.is_array()) {
      for (const auto& s : sessions) {
        bool auth = s.contains("auth") && s["auth"].is_boolean() && s["auth"].get<bool>();
        buf += jsonStr(s, "session_id") + "\t" + jsonStr(s, "session_name") + "\t" +
               jsonStr(s, "project_file") + "\t" + (auth ? "1" : "0") + "\n";
      }
    }
  }
  return buf.c_str();
}

// Recent activity, newest line last.
static const char* ReaMesh_GetLog() {
  static std::string buf;
  buf.clear();
  for (const std::string& line : g_log) {
    buf += line;
    buf += '\n';
  }
  return buf.c_str();
}

static bool ReaMesh_StartBrowse() { return startBrowse(); }

static bool ReaMesh_Host(const char* passphrase) {
  onHost(passphrase ? passphrase : "");
  return g_mode == Mode::Hosting;
}

static bool ReaMesh_JoinSession(const char* session_id, const char* passphrase) {
  if (!session_id || !*session_id) return false;
  return joinSessionById(session_id, "session", passphrase ? passphrase : "");
}

static void ReaMesh_Leave() {
  if (g_mode != Mode::Idle) {
    leaveSession("left the session");
    g_lastError.clear();
  }
}

// ReaScript vararg shims (see "APIvararg_*" in reaper_plugin.h).
static void* vararg_GetStatus(void**, int) { return (void*)ReaMesh_GetStatus(); }
static void* vararg_GetSessions(void**, int) { return (void*)ReaMesh_GetSessions(); }
static void* vararg_GetLog(void**, int) { return (void*)ReaMesh_GetLog(); }
static void* vararg_StartBrowse(void**, int) { return (void*)(INT_PTR)ReaMesh_StartBrowse(); }
static void* vararg_Host(void** arglist, int numparms) {
  const char* pass = numparms > 0 ? (const char*)arglist[0] : nullptr;
  return (void*)(INT_PTR)ReaMesh_Host(pass);
}
static void* vararg_JoinSession(void** arglist, int numparms) {
  const char* sid = numparms > 0 ? (const char*)arglist[0] : nullptr;
  const char* pass = numparms > 1 ? (const char*)arglist[1] : nullptr;
  return (void*)(INT_PTR)ReaMesh_JoinSession(sid, pass);
}
static void* vararg_Leave(void**, int) { ReaMesh_Leave(); return nullptr; }

struct ApiExport {
  const char* name;
  void* func;
  void* vararg;
  const char* def;  // four NUL-separated fields: ret \0 argtypes \0 argnames \0 help
};
static const ApiExport g_api[] = {
    {"ReaMesh_GetStatus", (void*)ReaMesh_GetStatus, (void*)vararg_GetStatus,
     "const char*\0\0\0Current ReaMesh state as key=value lines: mode (idle|browsing|downloading|hosting|client), state, session, progress (0..1), peers, path, error."},
    {"ReaMesh_GetSessions", (void*)ReaMesh_GetSessions, (void*)vararg_GetSessions,
     "const char*\0\0\0Sessions announced on the LAN, one per line: session_id TAB session_name TAB project_file TAB auth (1 if passphrase-protected, else 0). Requires browsing (see ReaMesh_StartBrowse)."},
    {"ReaMesh_GetLog", (void*)ReaMesh_GetLog, (void*)vararg_GetLog,
     "const char*\0\0\0Recent ReaMesh activity log, one event per line, newest last."},
    {"ReaMesh_StartBrowse", (void*)ReaMesh_StartBrowse, (void*)vararg_StartBrowse,
     "bool\0\0\0Start discovering ReaMesh sessions on the LAN. Idempotent while browsing."},
    {"ReaMesh_Host", (void*)ReaMesh_Host, (void*)vararg_Host,
     "bool\0const char*\0passphrase\0Host a ReaMesh session sharing the current project's folder. Pass a non-empty passphrase to require it of joiners (empty = open). Prompts to save first if the project is unsaved."},
    {"ReaMesh_JoinSession", (void*)ReaMesh_JoinSession, (void*)vararg_JoinSession,
     "bool\0const char*,const char*\0session_id,passphrase\0Join an announced ReaMesh session by id (see ReaMesh_GetSessions). Supply the passphrase if the session is protected (empty otherwise). Downloads the project, then opens it."},
    {"ReaMesh_Leave", (void*)ReaMesh_Leave, (void*)vararg_Leave,
     "void\0\0\0Leave the current ReaMesh session / stop hosting or browsing."},
};

static void registerApi(reaper_plugin_info_t* rec, bool add) {
  // REAPER may keep the registration key pointer; the constructed names must
  // outlive the call (deque: growth never relocates existing elements, which
  // would invalidate c_str() of SSO strings).
  static std::deque<std::string> keys;
  const char* prefix = add ? "" : "-";
  for (const ApiExport& a : g_api) {
    for (const char* kind : {"API_", "APIvararg_", "APIdef_"}) {
      keys.push_back(std::string(prefix) + kind + a.name);
      void* value = !strcmp(kind, "API_")        ? a.func
                    : !strcmp(kind, "APIvararg_") ? a.vararg
                                                  : (void*)a.def;
      rec->Register(keys.back().c_str(), value);
    }
  }
}

// ---------- GUI script install ----------

// Write the embedded ReaImGui panel script into the resource path and
// register it as an action. Returns the script's command id (0 on failure).
static int installGuiScript() {
  const fs::path dir = fs::path(GetResourcePath()) / "Scripts" / "ReaMesh";
  const fs::path path = dir / "ReaMesh GUI.lua";

  std::error_code ec;
  fs::create_directories(dir, ec);
  std::string existing;
  {
    std::ifstream in(path, std::ios::binary);
    existing.assign(std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>());
  }
  if (existing != kReameshGuiScript) {
    std::ofstream out(path, std::ios::binary | std::ios::trunc);
    out << kReameshGuiScript;
    if (!out) {
      logLine("could not write " + path.string() + ": GUI unavailable");
      return 0;
    }
  }
  return AddRemoveReaScript(true, 0, path.string().c_str(), true);
}

// ---------- timer ----------

static void pollDownload() {
  char* raw = reamesh_status(g_handle);
  std::string payload = raw ? raw : "{}";
  reamesh_free_string(raw);
  json st = json::parse(payload, nullptr, false);
  if (!st.is_object()) return;
  std::string state = st.value("state", "");

  if (state == "ready") {
    std::string path = st.value("project_path", "");
    logLine("download complete, opening " + path);
    g_applyingRemote = true;
    Main_openProject(path.c_str());
    // The loaded project IS the owner's state; baseline it so the load is
    // never diffed as a burst of local edits.
    g_snapshot = snapshotProject();
    g_lastChangeCount = GetProjectStateChangeCount(nullptr);
    g_applyingRemote = false;
    g_mode = Mode::ClientActive;
    g_sessionProjectPath = currentProjectPath();
    logLine("connected: your edits are proposals; the session owner wins conflicts");
  } else if (state == "failed") {
    leaveSession("join failed: " + st.value("message", std::string("unknown error")));
  }
}

static void timerTick() {
  if (g_applyingRemote || !g_handle) return;

  switch (g_mode) {
    case Mode::Idle:
    case Mode::Browsing:
      return;
    case Mode::Downloading:
      pollDownload();
      return;
    case Mode::Hosting:
    case Mode::ClientActive: {
      // Switching projects ends the session: its doc is bound to the shared
      // project file.
      if (currentProjectPath() != g_sessionProjectPath) {
        leaveSession("project changed; session ended");
        return;
      }
      unsigned peers = reamesh_peer_count(g_handle);
      if (peers != g_lastPeerCount) {
        logLine(peers > g_lastPeerCount ? "a peer joined the session"
                                        : "a peer left the session");
        g_lastPeerCount = peers;
      }
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
  if (command == g_cmdJoin) { openPanel(); return true; }
  if (command == g_cmdLeave) { onLeave(); return true; }
  if (command == g_cmdPanel) { openPanel(); return true; }
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
      g_rec->Register("-custom_action", &g_caJoin);
      g_rec->Register("-custom_action", &g_caLeave);
      g_rec->Register("-custom_action", &g_caPanel);
      registerApi(g_rec, false);
    }
    leaveSession("");
    g_rec = nullptr;
    return 0;
  }
  if (rec->caller_version != REAPER_PLUGIN_VERSION || !rec->GetFunc) return 0;
  if (REAPERAPI_LoadAPI(rec->GetFunc) != 0) return 0;

  g_rec = rec;
  g_cmdHost = rec->Register("custom_action", &g_caHost);
  g_cmdJoin = rec->Register("custom_action", &g_caJoin);
  g_cmdLeave = rec->Register("custom_action", &g_caLeave);
  g_cmdPanel = rec->Register("custom_action", &g_caPanel);
  rec->Register("hookcommand", (void*)hookCommand);
  rec->Register("hookcommand2", (void*)hookCommand2);
  rec->Register("timer", (void*)timerTick);
  registerApi(rec, true);
  g_guiScriptCmd = installGuiScript();
  return 1;
}
