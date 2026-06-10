// FFI smoke test: drives the Rust library through the exact C ABI the REAPER
// extension uses, with two in-process peers over loopback. No REAPER needed.
//
// Build & run (from the repo root, after ./build.sh):
//   clang++ -std=c++20 -Iextension/include tools/ffi_smoke.cpp \
//     rust/target/release/libreamesh.a \
//     -framework CoreFoundation -framework Security \
//     -framework SystemConfiguration -lresolv -o /tmp/ffi_smoke && /tmp/ffi_smoke

#include "reamesh_ffi.h"

#include <chrono>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>

static bool drainUntil(ReameshHandle* h, const char* needle, int seconds) {
  auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(seconds);
  while (std::chrono::steady_clock::now() < deadline) {
    char* raw = reamesh_drain_remote(h);
    std::string out = raw ? raw : "";
    reamesh_free_string(raw);
    if (out.find(needle) != std::string::npos) {
      printf("  drained: %s\n", out.c_str());
      return true;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
  }
  return false;
}

int main() {
  printf("init peer A...\n");
  ReameshHandle* a = reamesh_init("ffi-smoke");
  printf("init peer B...\n");
  ReameshHandle* b = reamesh_init("ffi-smoke");
  if (!a || !b) {
    fprintf(stderr, "FAIL: init returned null\n");
    return 1;
  }

  printf("waiting for discovery...\n");
  auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(15);
  while (std::chrono::steady_clock::now() < deadline &&
         (reamesh_peer_count(a) == 0 || reamesh_peer_count(b) == 0)) {
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
  }
  if (reamesh_peer_count(a) == 0 || reamesh_peer_count(b) == 0) {
    fprintf(stderr, "FAIL: peers never discovered each other (is multicast blocked?)\n");
    return 1;
  }
  printf("peers connected (A sees %u, B sees %u)\n",
         reamesh_peer_count(a), reamesh_peer_count(b));

  printf("A pushes a track...\n");
  reamesh_push_local(a,
      "{\"tracks\":{\"{SMOKE-1}\":{\"present\":true,\"index\":0,\"name\":\"Vox\","
      "\"vol\":0.8,\"pan\":0.1,\"mute\":false,\"solo\":false}}}");
  if (!drainUntil(b, "Vox", 10)) {
    fprintf(stderr, "FAIL: B never drained A's track\n");
    return 1;
  }

  printf("B renames it...\n");
  reamesh_push_local(b, "{\"tracks\":{\"{SMOKE-1}\":{\"name\":\"Lead Vox\"}}}");
  if (!drainUntil(a, "Lead Vox", 10)) {
    fprintf(stderr, "FAIL: A never drained B's rename\n");
    return 1;
  }

  // No echo: both queues must be quiet now.
  std::this_thread::sleep_for(std::chrono::milliseconds(500));
  char* ra = reamesh_drain_remote(a);
  char* rb = reamesh_drain_remote(b);
  bool quiet = strcmp(ra, "[]") == 0 && strcmp(rb, "[]") == 0;
  if (!quiet) fprintf(stderr, "FAIL: echo detected (A: %s, B: %s)\n", ra, rb);
  reamesh_free_string(ra);
  reamesh_free_string(rb);
  if (!quiet) return 1;

  reamesh_shutdown(a);
  reamesh_shutdown(b);
  printf("OK: FFI round trip, both directions, no echo\n");
  return 0;
}
