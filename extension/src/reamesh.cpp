// ReaMesh REAPER extension: thin, synchronous capture/apply layer.
//
// Everything in this file runs on REAPER's main thread, driven by the
// registered timer. The Rust library (reamesh_ffi.h) owns all networking and
// CRDT state on its own background threads; we only exchange JSON changeset
// strings with it and never block on the network.

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
#define REAPERAPI_WANT_ShowConsoleMsg
#define REAPERAPI_WANT_EnumProjects

#include "reaper_plugin.h"
#include "reaper_plugin_functions.h"

#include "reamesh_ffi.h"
#include "json.hpp"

#include <algorithm>
#include <cstring>
#include <map>
#include <string>
#include <vector>

using nlohmann::json;

// ---------- state ----------

struct TrackState {
  int index = 0;
  std::string name;
  double vol = 1.0;
  double pan = 0.0;
  bool mute = false;
  bool solo = false;
  bool operator==(const TrackState&) const = default;
};

static reaper_plugin_info_t* g_rec = nullptr;
static ReameshHandle* g_handle = nullptr;
static std::string g_projectId;
static std::map<std::string, TrackState> g_snapshot;  // GUID -> last known state
static int g_lastChangeCount = -1;
static bool g_applyingRemote = false;  // echo-suppression / reentrancy guard
static int g_initRetryTicks = 0;
static unsigned g_lastPeerCount = 0;

static void msg(const std::string& s) {
  if (ShowConsoleMsg) ShowConsoleMsg(("[ReaMesh] " + s + "\n").c_str());
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

static std::map<std::string, TrackState> snapshotProject() {
  std::map<std::string, TrackState> snap;
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
    snap[trackGuid(tr)] = ts;
  }
  return snap;
}

static json trackToJson(const TrackState& ts) {
  return json{{"present", true}, {"index", ts.index}, {"name", ts.name},
              {"vol", ts.vol},   {"pan", ts.pan},     {"mute", ts.mute},
              {"solo", ts.solo}};
}

// Minimal changeset: full field set for new tracks, changed fields only for
// known tracks, a present:false tombstone for removed ones. Empty string when
// nothing relevant changed.
static std::string diffSnapshots(const std::map<std::string, TrackState>& prev,
                                 const std::map<std::string, TrackState>& now) {
  json tracks = json::object();
  for (const auto& [guid, ts] : now) {
    auto it = prev.find(guid);
    if (it == prev.end()) {
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
    if (!delta.empty()) tracks[guid] = delta;
  }
  for (const auto& [guid, ts] : prev) {
    if (!now.count(guid)) tracks[guid] = json{{"present", false}};
  }
  if (tracks.empty()) return "";
  return json{{"tracks", tracks}}.dump();
}

static void captureLocalChanges() {
  const int cc = GetProjectStateChangeCount(nullptr);
  if (cc == g_lastChangeCount) return;
  g_lastChangeCount = cc;

  auto now = snapshotProject();
  std::string changeset = diffSnapshots(g_snapshot, now);
  g_snapshot = std::move(now);
  if (changeset.empty()) return;  // change count moved for something we don't sync

  reamesh_push_local(g_handle, changeset.c_str());
  msg("push " + (changeset.size() > 400 ? changeset.substr(0, 400) + "..." : changeset));
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

static void applyChangeset(const json& changeset) {
  if (!changeset.contains("tracks") || !changeset["tracks"].is_object()) return;
  const json& tracks = changeset["tracks"];

  // Deletions first so insertion indices below land in a settled track list.
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
      // Stamp the originator's GUID onto the new track so identity is shared
      // across all peers (indices are never used as identity).
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
  PreventUIRefresh(1);
  for (const auto& cs : changesets) applyChangeset(cs);
  PreventUIRefresh(-1);
  TrackList_AdjustWindows(false);
  UpdateArrange();

  g_snapshot = snapshotProject();
  g_lastChangeCount = GetProjectStateChangeCount(nullptr);
  g_applyingRemote = false;

  msg("applied " + (payload.size() > 400 ? payload.substr(0, 400) + "..." : payload));
}

// ---------- session lifecycle ----------

static std::string projectIdFromPath(const char* path) {
  std::string p = path ? path : "";
  if (p.empty()) return "untitled";
  size_t slash = p.find_last_of("/\\");
  if (slash != std::string::npos) p = p.substr(slash + 1);
  size_t dot = p.find_last_of('.');
  if (dot != std::string::npos && dot > 0) p = p.substr(0, dot);
  return p.empty() ? "untitled" : p;
}

// (Re)create the sync session whenever the active project file changes. The
// gossip topic is derived from the project id, so peers only see each other
// when they have the same project open.
static void ensureSession() {
  char fn[2048] = "";
  EnumProjects(-1, fn, sizeof(fn));
  std::string pid = projectIdFromPath(fn);
  if (pid == g_projectId) {
    if (g_handle) return;
    if (g_initRetryTicks > 0) {  // last init failed; back off before retrying
      --g_initRetryTicks;
      return;
    }
  }

  if (g_handle) {
    reamesh_shutdown(g_handle);
    g_handle = nullptr;
  }
  g_projectId = pid;
  g_handle = reamesh_init(pid.c_str());
  g_snapshot.clear();
  g_lastChangeCount = -1;  // force a full capture; this publishes our project state
  g_lastPeerCount = 0;
  if (g_handle) {
    msg("session \"" + pid + "\" started, looking for peers on the LAN...");
  } else {
    msg("session \"" + pid + "\" FAILED to start (see stderr); will retry");
    g_initRetryTicks = 150;  // ~5s at the ~30 Hz timer rate
  }
}

// ---------- timer ----------

static void timerTick() {
  if (g_applyingRemote) return;
  ensureSession();
  if (!g_handle) return;

  unsigned peers = reamesh_peer_count(g_handle);
  if (peers != g_lastPeerCount) {
    msg("peers in session: " + std::to_string(peers));
    g_lastPeerCount = peers;
  }

  // Capture before apply, so an edit made by the user in this tick interval
  // is diffed against the old baseline before remote application re-baselines.
  captureLocalChanges();
  applyRemoteChanges();
}

// ---------- entry point ----------

extern "C" REAPER_PLUGIN_DLL_EXPORT int REAPER_PLUGIN_ENTRYPOINT(
    REAPER_PLUGIN_HINSTANCE hInstance, reaper_plugin_info_t* rec) {
  (void)hInstance;
  if (!rec) {
    // Plugin unloading.
    if (g_rec) g_rec->Register("-timer", (void*)timerTick);
    if (g_handle) {
      reamesh_shutdown(g_handle);
      g_handle = nullptr;
    }
    g_rec = nullptr;
    return 0;
  }
  if (rec->caller_version != REAPER_PLUGIN_VERSION || !rec->GetFunc) return 0;
  if (REAPERAPI_LoadAPI(rec->GetFunc) != 0) return 0;

  g_rec = rec;
  rec->Register("timer", (void*)timerTick);
  msg("loaded (LAN co-editing of track add/remove/order, name, vol, pan, mute, solo)");
  return 1;
}
