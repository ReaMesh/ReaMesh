// ReaMesh REAPER extension: thin, synchronous capture/apply layer.
//
// Everything in this file runs on REAPER's main thread, driven by the
// registered timer and action hooks. The Rust library (reamesh_ffi.h) owns
// all networking, the CRDT doc and the project-file transfer on its own
// background threads; we only exchange JSON strings with it and never block
// on the network.
//
// v2 session model (owner-authoritative):
//   "ReaMesh: Host session":   share the saved project (files included) on
//                              the LAN; this peer's edits win conflicts.
//   "ReaMesh: Join session":   discover sessions, download the project into
//                              <resource>/ReaMesh/, open it, collaborate.
//   "ReaMesh: Leave session":  stop hosting / leave.
//   "ReaMesh: Open panel (GUI)": ReaImGui panel (scripts/ReaMesh GUI.lua,
//                              embedded at build time, auto-installed into
//                              <resource>/Scripts/ReaMesh/ and driven through
//                              the ReaMesh_* ReaScript API below).

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
#define REAPERAPI_WANT_Main_SaveProject
#define REAPERAPI_WANT_Main_openProject
#define REAPERAPI_WANT_GetResourcePath
#define REAPERAPI_WANT_AddRemoveReaScript
#define REAPERAPI_WANT_Main_OnCommand

#include "reaper_plugin.h"
#include "reaper_plugin_functions.h"

#include "reamesh_ffi.h"
#include "reamesh_gui_script.h"
#include "json.hpp"

#include <algorithm>
#include <cstring>
#include <deque>
#include <filesystem>
#include <fstream>
#include <iterator>
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

enum class Mode { Idle, Hosting, Browsing, Downloading, ClientActive };

static reaper_plugin_info_t* g_rec = nullptr;
static ReameshHandle* g_handle = nullptr;
static Mode g_mode = Mode::Idle;
static std::string g_sessionProjectPath;  // path the session is bound to
static std::map<std::string, TrackState> g_snapshot;  // GUID -> last known state
static int g_lastChangeCount = -1;
static bool g_applyingRemote = false;  // echo-suppression / reentrancy guard
static unsigned g_lastPeerCount = 0;
static int g_lastProgressPct = -1;
static std::string g_lastError;  // last session-ending message, shown by the GUI
static int g_guiScriptCmd = 0;   // command id of the installed GUI ReaScript

static int g_cmdHost = 0, g_cmdJoin = 0, g_cmdLeave = 0, g_cmdPanel = 0;
static custom_action_register_t g_caHost = {
    0, "REAMESH_HOST", "ReaMesh: Host session (share current project on LAN)", nullptr};
static custom_action_register_t g_caJoin = {
    0, "REAMESH_JOIN", "ReaMesh: Join session (download project from LAN)", nullptr};
static custom_action_register_t g_caLeave = {
    0, "REAMESH_LEAVE", "ReaMesh: Leave session / stop hosting", nullptr};
static custom_action_register_t g_caPanel = {
    0, "REAMESH_PANEL", "ReaMesh: Open panel (GUI)", nullptr};

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
  g_snapshot.clear();
  g_lastChangeCount = -1;
  g_lastPeerCount = 0;
  g_lastProgressPct = -1;
  if (!why.empty()) {
    msg(why);
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
    msg("FAILED to start session discovery (see stderr).");
    g_lastError = "could not start session discovery (see stderr)";
    return false;
  }
  g_mode = Mode::Browsing;
  g_lastError.clear();
  return true;
}

// Join one of the announced sessions (Browsing -> Downloading).
static bool joinSessionById(const std::string& sid, const std::string& sname) {
  if (g_mode != Mode::Browsing || !g_handle) return false;
  if (!reamesh_join_session(g_handle, sid.c_str())) {
    msg("could not join \"" + sname + "\" (it may have just closed); try again.");
    return false;
  }
  g_mode = Mode::Downloading;
  g_lastProgressPct = -1;
  g_lastError.clear();
  msg("joining \"" + sname + "\", downloading project files...");
  return true;
}

static void onHost() {
  if (g_mode == Mode::Hosting) {
    msg("already hosting; use \"ReaMesh: Leave session\" first.");
    return;
  }
  if (g_mode != Mode::Idle) leaveSession("left previous session.");

  std::string path = currentProjectPath();
  if (path.empty()) {
    // Unsaved project: trigger Save As so there is a project file to share.
    msg("project is unsaved; choose where to save it, then run Host again.");
    Main_SaveProject(nullptr, true);
    return;
  }
  Main_SaveProject(nullptr, false);  // share what's on screen, not a stale file

  size_t slash = path.find_last_of("/\\");
  std::string dir = (slash != std::string::npos) ? path.substr(0, slash) : ".";
  std::string file = (slash != std::string::npos) ? path.substr(slash + 1) : path;
  std::string name = fileStem(file);

  g_handle = reamesh_host(name.c_str(), dir.c_str(), file.c_str());
  if (!g_handle) {
    msg("FAILED to start hosting (see stderr).");
    g_lastError = "could not start hosting (see stderr)";
    return;
  }
  g_mode = Mode::Hosting;
  g_lastError.clear();
  g_sessionProjectPath = path;
  g_snapshot.clear();
  g_lastChangeCount = -1;  // force a full capture; this seeds the shared doc
  g_lastPeerCount = 0;
  msg("hosting session \"" + name + "\", sharing all files under " + dir);
  msg("(make sure media is in the project folder: File > Save project, \"Copy all media\")");
}

static void onJoin() {
  if (g_mode == Mode::Hosting || g_mode == Mode::ClientActive) {
    msg("already in a session; use \"ReaMesh: Leave session\" first.");
    return;
  }
  if (g_mode == Mode::Idle) {
    if (startBrowse())
      msg("searching for sessions on the LAN... run \"Join session\" again in a few seconds.");
    return;
  }
  if (g_mode == Mode::Downloading) {
    msg("already joining a session, hang on...");
    return;
  }

  // Browsing: list what we found and join the first session.
  char* raw = reamesh_sessions(g_handle);
  std::string payload = raw ? raw : "[]";
  reamesh_free_string(raw);
  json sessions = json::parse(payload, nullptr, false);
  if (!sessions.is_array() || sessions.empty()) {
    msg("no sessions found yet. Is someone hosting on this network? Try again shortly.");
    return;
  }
  for (const auto& s : sessions)
    msg("found session \"" + s.value("session_name", "?") + "\" (" +
        s.value("project_file", "?") + ")");
  std::string sid = sessions[0].value("session_id", "");
  std::string sname = sessions[0].value("session_name", "?");
  if (sessions.size() > 1)
    msg("multiple sessions found; joining the first: \"" + sname + "\"");
  joinSessionById(sid, sname);
}

static void onLeave() {
  if (g_mode == Mode::Idle) {
    msg("not in a session.");
    return;
  }
  leaveSession("left the session.");
  g_lastError.clear();  // user-initiated, nothing to flag in the GUI
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
      for (const auto& s : sessions)
        buf += jsonStr(s, "session_id") + "\t" + jsonStr(s, "session_name") + "\t" +
               jsonStr(s, "project_file") + "\n";
    }
  }
  return buf.c_str();
}

static bool ReaMesh_StartBrowse() { return startBrowse(); }

static bool ReaMesh_Host() {
  onHost();
  return g_mode == Mode::Hosting;
}

static bool ReaMesh_JoinSession(const char* session_id) {
  if (!session_id || !*session_id) return false;
  return joinSessionById(session_id, "session");
}

static void ReaMesh_Leave() {
  if (g_mode != Mode::Idle) {
    leaveSession("left the session.");
    g_lastError.clear();
  }
}

// ReaScript vararg shims (see "APIvararg_*" in reaper_plugin.h).
static void* vararg_GetStatus(void**, int) { return (void*)ReaMesh_GetStatus(); }
static void* vararg_GetSessions(void**, int) { return (void*)ReaMesh_GetSessions(); }
static void* vararg_StartBrowse(void**, int) { return (void*)(INT_PTR)ReaMesh_StartBrowse(); }
static void* vararg_Host(void**, int) { return (void*)(INT_PTR)ReaMesh_Host(); }
static void* vararg_JoinSession(void** arglist, int numparms) {
  const char* sid = numparms > 0 ? (const char*)arglist[0] : nullptr;
  return (void*)(INT_PTR)ReaMesh_JoinSession(sid);
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
     "const char*\0\0\0Sessions announced on the LAN, one per line: session_id TAB session_name TAB project_file. Requires browsing (see ReaMesh_StartBrowse)."},
    {"ReaMesh_StartBrowse", (void*)ReaMesh_StartBrowse, (void*)vararg_StartBrowse,
     "bool\0\0\0Start discovering ReaMesh sessions on the LAN. Idempotent while browsing."},
    {"ReaMesh_Host", (void*)ReaMesh_Host, (void*)vararg_Host,
     "bool\0\0\0Host a ReaMesh session sharing the current project's folder. Prompts to save first if the project is unsaved."},
    {"ReaMesh_JoinSession", (void*)ReaMesh_JoinSession, (void*)vararg_JoinSession,
     "bool\0const char*\0session_id\0Join an announced ReaMesh session by id (see ReaMesh_GetSessions). Downloads the project, then opens it."},
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
  namespace fs = std::filesystem;
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
      msg("could not write " + path.string() + ": GUI unavailable");
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

  if (state == "downloading") {
    int pct = (int)(st.value("progress", 0.0) * 100.0);
    if (pct / 10 > g_lastProgressPct / 10) {
      msg("downloading... " + std::to_string(pct) + "%");
      g_lastProgressPct = pct;
    }
  } else if (state == "ready") {
    std::string path = st.value("project_path", "");
    msg("download complete, opening " + path);
    g_applyingRemote = true;
    Main_openProject(path.c_str());
    // The loaded project IS the owner's state; baseline it so the load is
    // never diffed as a burst of local edits.
    g_snapshot = snapshotProject();
    g_lastChangeCount = GetProjectStateChangeCount(nullptr);
    g_applyingRemote = false;
    g_mode = Mode::ClientActive;
    g_sessionProjectPath = currentProjectPath();
    msg("connected: your edits are proposals; the session owner wins conflicts.");
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
        leaveSession("project changed; session ended.");
        return;
      }
      unsigned peers = reamesh_peer_count(g_handle);
      if (peers != g_lastPeerCount) {
        msg("peers in session: " + std::to_string(peers));
        g_lastPeerCount = peers;
      }
      // Capture before apply, so an edit made by the user in this tick
      // interval is diffed against the old baseline before remote
      // application re-baselines.
      captureLocalChanges();
      applyRemoteChanges();
      return;
    }
  }
}

// ---------- actions / entry point ----------

static bool dispatchCommand(int command) {
  if (command == 0) return false;
  if (command == g_cmdHost) { onHost(); return true; }
  if (command == g_cmdJoin) { onJoin(); return true; }
  if (command == g_cmdLeave) { onLeave(); return true; }
  if (command == g_cmdPanel) {
    if (g_guiScriptCmd)
      Main_OnCommand(g_guiScriptCmd, 0);
    else
      msg("GUI script unavailable (could not be installed at startup).");
    return true;
  }
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
  msg("loaded; run the action \"ReaMesh: Open panel (GUI)\" to host or join a session"
      " (needs the ReaImGui extension, installable via ReaPack).");
  return 1;
}
