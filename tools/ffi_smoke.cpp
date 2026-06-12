// FFI smoke test: drives the Rust library through the exact C ABI the REAPER
// extension uses: one owner hosting a temp project, one client joining over
// loopback, downloading the files and exchanging edits. No REAPER needed.
//
// Build & run (from the repo root, after ./build.sh):
//   clang++ -std=c++20 -Iextension/include tools/ffi_smoke.cpp \
//     rust/target/release/libreamesh.a \
//     -framework CoreFoundation -framework Security \
//     -framework SystemConfiguration -lresolv -o /tmp/ffi_smoke && /tmp/ffi_smoke

#include "reamesh_ffi.h"

#include <unistd.h>

#include <chrono>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <string>
#include <thread>

namespace fs = std::filesystem;

static std::string take(char* raw) {
  std::string out = raw ? raw : "";
  reamesh_free_string(raw);
  return out;
}

static bool drainUntil(ReameshHandle* h, const char* needle, int seconds) {
  auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(seconds);
  while (std::chrono::steady_clock::now() < deadline) {
    std::string out = take(reamesh_drain_remote(h));
    if (out.find(needle) != std::string::npos) {
      printf("  drained: %s\n", out.c_str());
      return true;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
  }
  return false;
}

// Extract a top-level string value from a flat JSON object (good enough for
// the status/session payloads this test reads).
static std::string jsonStr(const std::string& js, const std::string& key) {
  std::string pat = "\"" + key + "\":\"";
  size_t p = js.find(pat);
  if (p == std::string::npos) return "";
  p += pat.size();
  size_t e = js.find('"', p);
  return e == std::string::npos ? "" : js.substr(p, e - p);
}

int main() {
  const auto base = fs::temp_directory_path() / ("reamesh-smoke-" + std::to_string(getpid()));
  const auto proj = base / "project";
  const auto staging = base / "staging";
  fs::create_directories(proj / "Audio");
  fs::create_directories(staging);
  std::ofstream(proj / "Smoke.rpp") << "<REAPER_PROJECT 0.1\n>\n";
  {
    std::ofstream wav(proj / "Audio" / "kick.wav", std::ios::binary);
    for (int i = 0; i < 1500000; ++i) wav.put((char)(i % 251));
  }

  printf("hosting...\n");
  ReameshHandle* owner = reamesh_host("Smoke", proj.c_str(), "Smoke.rpp", "");
  printf("browsing...\n");
  ReameshHandle* client = reamesh_join(staging.c_str());
  if (!owner || !client) {
    fprintf(stderr, "FAIL: init returned null\n");
    return 1;
  }
  std::string sid = jsonStr(take(reamesh_status(owner)), "session_id");

  printf("waiting for the client to discover session %s...\n", sid.c_str());
  auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(20);
  bool found = false;
  while (std::chrono::steady_clock::now() < deadline) {
    if (take(reamesh_sessions(client)).find(sid) != std::string::npos) { found = true; break; }
    std::this_thread::sleep_for(std::chrono::milliseconds(200));
  }
  if (!found) {
    fprintf(stderr, "FAIL: session never discovered (is multicast blocked?)\n");
    return 1;
  }

  printf("joining + downloading...\n");
  if (!reamesh_join_session(client, sid.c_str(), "")) {
    fprintf(stderr, "FAIL: join_session refused\n");
    return 1;
  }
  std::string projectPath;
  deadline = std::chrono::steady_clock::now() + std::chrono::seconds(30);
  while (std::chrono::steady_clock::now() < deadline) {
    std::string st = take(reamesh_status(client));
    std::string state = jsonStr(st, "state");
    if (state == "ready") { projectPath = jsonStr(st, "project_path"); break; }
    if (state == "failed") {
      fprintf(stderr, "FAIL: download failed: %s\n", st.c_str());
      return 1;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
  }
  if (projectPath.empty()) {
    fprintf(stderr, "FAIL: download never completed\n");
    return 1;
  }
  if (!fs::exists(projectPath) ||
      fs::file_size(fs::path(projectPath).parent_path() / "Audio" / "kick.wav") != 1500000) {
    fprintf(stderr, "FAIL: downloaded project incomplete at %s\n", projectPath.c_str());
    return 1;
  }
  printf("project downloaded to %s\n", projectPath.c_str());

  printf("waiting for the session mesh...\n");
  deadline = std::chrono::steady_clock::now() + std::chrono::seconds(15);
  while (std::chrono::steady_clock::now() < deadline &&
         (reamesh_peer_count(owner) == 0 || reamesh_peer_count(client) == 0)) {
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
  }
  if (reamesh_peer_count(owner) == 0 || reamesh_peer_count(client) == 0) {
    fprintf(stderr, "FAIL: session mesh never formed\n");
    return 1;
  }

  printf("owner pushes a track...\n");
  reamesh_push_local(owner,
      "{\"tracks\":{\"{SMOKE-1}\":{\"present\":true,\"index\":0,\"name\":\"Vox\","
      "\"vol\":0.8,\"pan\":0.1,\"mute\":false,\"fxen\":true}}}");
  if (!drainUntil(client, "Vox", 10)) {
    fprintf(stderr, "FAIL: client never drained the owner's track\n");
    return 1;
  }

  printf("client proposes a rename (after the owner-priority window)...\n");
  std::this_thread::sleep_for(std::chrono::milliseconds(3200));
  reamesh_push_local(client, "{\"tracks\":{\"{SMOKE-1}\":{\"name\":\"Lead Vox\"}}}");
  if (!drainUntil(owner, "Lead Vox", 10)) {
    fprintf(stderr, "FAIL: owner never drained the client's proposal\n");
    return 1;
  }

  // No echo: both queues must be quiet once the proposal is confirmed
  // (confirmation is swallowed client-side, application re-baselined
  // owner-side).
  std::this_thread::sleep_for(std::chrono::milliseconds(4000));
  std::string ra = take(reamesh_drain_remote(owner));
  std::string rb = take(reamesh_drain_remote(client));
  if (ra != "[]" || rb != "[]") {
    fprintf(stderr, "FAIL: echo detected (owner: %s, client: %s)\n", ra.c_str(), rb.c_str());
    return 1;
  }

  printf("owner pushes a media item...\n");
  reamesh_push_local(owner,
      "{\"items\":{\"{SMOKE-I1}\":{\"present\":true,\"track\":\"{SMOKE-1}\","
      "\"chunk\":\"<ITEM\\nPOSITION 2.5\\n>\"}}}");
  if (!drainUntil(client, "POSITION 2.5", 10)) {
    fprintf(stderr, "FAIL: client never drained the owner's item\n");
    return 1;
  }

  printf("client fetches media that appeared mid-session...\n");
  {
    std::ofstream wav(proj / "Audio" / "snare.wav", std::ios::binary);
    for (int i = 0; i < 300000; ++i) wav.put((char)(i % 241));
  }
  const fs::path clientSnare = fs::path(projectPath).parent_path() / "Audio" / "snare.wav";
  deadline = std::chrono::steady_clock::now() + std::chrono::seconds(20);
  while (std::chrono::steady_clock::now() < deadline && !fs::exists(clientSnare)) {
    reamesh_fetch_file(client, "Audio/snare.wav");
    std::this_thread::sleep_for(std::chrono::milliseconds(200));
  }
  if (!fs::exists(clientSnare) || fs::file_size(clientSnare) != 300000) {
    fprintf(stderr, "FAIL: mid-session media fetch never completed\n");
    return 1;
  }

  reamesh_shutdown(client);
  reamesh_shutdown(owner);
  fs::remove_all(base);
  printf("OK: discovery, file transfer, track + item round trips, media fetch, no echo\n");
  return 0;
}
