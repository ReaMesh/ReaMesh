cmake_minimum_required(VERSION 3.20)
project(reaper_reamesh CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_library(reaper_reamesh SHARED src/reamesh.cpp)
set_target_properties(reaper_reamesh PROPERTIES
  PREFIX ""
  # Only export ReaperPluginEntry (marked with REAPER_PLUGIN_DLL_EXPORT);
  # keeps the REAPER API pointer globals from colliding with other extensions.
  CXX_VISIBILITY_PRESET hidden
  VISIBILITY_INLINES_HIDDEN ON
)
if(APPLE)
  set_target_properties(reaper_reamesh PROPERTIES SUFFIX ".dylib")
endif()

target_include_directories(reaper_reamesh PRIVATE include sdk)

if(MSVC)
  # Compile source files and string literals as UTF-8.
  target_compile_options(reaper_reamesh PRIVATE /utf-8)
endif()

# Rust static library, built by build.sh or cargo before CMake runs.
if(WIN32)
  set(REAMESH_RUST_LIB_DEFAULT "${CMAKE_CURRENT_SOURCE_DIR}/../rust/target/release/reamesh.lib")
else()
  set(REAMESH_RUST_LIB_DEFAULT "${CMAKE_CURRENT_SOURCE_DIR}/../rust/target/release/libreamesh.a")
endif()
set(REAMESH_RUST_LIB "${REAMESH_RUST_LIB_DEFAULT}"
    CACHE FILEPATH "Path to the Rust staticlib")
if(NOT EXISTS "${REAMESH_RUST_LIB}")
  message(FATAL_ERROR "Rust library not found at ${REAMESH_RUST_LIB}; run 'cargo build --release' in rust/ first (or use ../build.sh)")
endif()
target_link_libraries(reaper_reamesh PRIVATE "${REAMESH_RUST_LIB}")

# System frameworks/libs the Rust crates (libp2p, tokio, rustls, mDNS) rely on.
if(APPLE)
  target_link_libraries(reaper_reamesh PRIVATE
    "-framework CoreFoundation"
    "-framework CoreGraphics"  # main-display height for the overlay y-flip
    "-framework Security"
    "-framework SystemConfiguration"
    resolv
  )
elseif(WIN32)
  target_link_libraries(reaper_reamesh PRIVATE
    ws2_32 bcrypt ntdll userenv iphlpapi crypt32 secur32 ncrypt runtimeobject propsys
  )
endif()
