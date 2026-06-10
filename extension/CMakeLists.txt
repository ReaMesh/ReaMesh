cmake_minimum_required(VERSION 3.20)
project(reaper_reamesh CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_library(reaper_reamesh SHARED src/reamesh.cpp)
set_target_properties(reaper_reamesh PROPERTIES
  PREFIX ""
  SUFFIX ".dylib"
  # Only export ReaperPluginEntry (marked with REAPER_PLUGIN_DLL_EXPORT);
  # keeps the REAPER API pointer globals from colliding with other extensions.
  CXX_VISIBILITY_PRESET hidden
  VISIBILITY_INLINES_HIDDEN ON
)

target_include_directories(reaper_reamesh PRIVATE include sdk)

# The Rust staticlib (built by ../build.sh before cmake runs).
set(REAMESH_RUST_LIB "${CMAKE_CURRENT_SOURCE_DIR}/../rust/target/release/libreamesh.a"
    CACHE FILEPATH "Path to the Rust staticlib")
if(NOT EXISTS "${REAMESH_RUST_LIB}")
  message(FATAL_ERROR "Rust library not found at ${REAMESH_RUST_LIB}; run 'cargo build --release' in rust/ first (or use ../build.sh)")
endif()
target_link_libraries(reaper_reamesh PRIVATE "${REAMESH_RUST_LIB}")

# System frameworks/libs the Rust crates (libp2p, tokio, rustls, mDNS) rely on.
if(APPLE)
  target_link_libraries(reaper_reamesh PRIVATE
    "-framework CoreFoundation"
    "-framework Security"
    "-framework SystemConfiguration"
    resolv
  )
endif()
