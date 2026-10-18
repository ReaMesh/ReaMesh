cmake_minimum_required(VERSION 3.20)

# macOS: one universal binary (Apple silicon + Intel) for macOS 11 and later,
# matching the app's minimumSystemVersion. Both must be known before project().
# Without a deployment target clang stamps the build machine's macOS version
# as the minimum, and an older macOS refuses to load the extension.
if(CMAKE_HOST_SYSTEM_NAME STREQUAL "Darwin")
  set(CMAKE_OSX_ARCHITECTURES "arm64;x86_64" CACHE STRING "macOS architectures")
  set(CMAKE_OSX_DEPLOYMENT_TARGET "11.0" CACHE STRING "Minimum macOS version")
endif()

project(reaper_reamesh CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

if(APPLE)
  enable_language(OBJCXX)
  set(CMAKE_OBJCXX_STANDARD 20)
  set(CMAKE_OBJCXX_STANDARD_REQUIRED ON)
  set(REAMESH_CURSOR_SRC src/cursor_overlay_mac.mm)
  set_source_files_properties(src/cursor_overlay_mac.mm PROPERTIES COMPILE_FLAGS "-fobjc-arc")
elseif(WIN32)
  set(REAMESH_CURSOR_SRC src/cursor_overlay_win.cpp)
else()
  set(REAMESH_CURSOR_SRC src/cursor_overlay_stub.cpp)
endif()

add_library(reaper_reamesh SHARED src/reamesh.cpp ${REAMESH_CURSOR_SRC})
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

# Rust static library, built by build.sh or cargo before CMake runs. A
# universal macOS build needs the lipo'd library build.sh puts together.
if(WIN32)
  set(REAMESH_RUST_LIB_DEFAULT "${CMAKE_CURRENT_SOURCE_DIR}/../rust/target/release/reamesh.lib")
elseif(APPLE AND CMAKE_OSX_ARCHITECTURES MATCHES ";")
  set(REAMESH_RUST_LIB_DEFAULT "${CMAKE_CURRENT_SOURCE_DIR}/../rust/target/universal-apple-darwin/release/libreamesh.a")
else()
  set(REAMESH_RUST_LIB_DEFAULT "${CMAKE_CURRENT_SOURCE_DIR}/../rust/target/release/libreamesh.a")
endif()
set(REAMESH_RUST_LIB "${REAMESH_RUST_LIB_DEFAULT}"
    CACHE FILEPATH "Path to the Rust staticlib")
if(NOT EXISTS "${REAMESH_RUST_LIB}")
  message(FATAL_ERROR "Rust library not found at ${REAMESH_RUST_LIB}; run ../build.sh first (or 'cargo build --release' in rust/ for a single-architecture build)")
endif()
target_link_libraries(reaper_reamesh PRIVATE "${REAMESH_RUST_LIB}")

# System frameworks/libs the Rust crates (libp2p, tokio, rustls, mDNS) rely on.
if(APPLE)
  target_link_libraries(reaper_reamesh PRIVATE
    "-framework CoreFoundation"
    "-framework CoreGraphics"
    "-framework Cocoa"         # peer cursor layer (cursor_overlay_mac.mm)
    "-framework QuartzCore"
    "-framework Security"
    "-framework SystemConfiguration"
    resolv
  )
elseif(WIN32)
  target_link_libraries(reaper_reamesh PRIVATE
    ws2_32 bcrypt ntdll userenv iphlpapi crypt32 secur32 ncrypt runtimeobject propsys
    # peer cursor layer (cursor_overlay_win.cpp); dcomp.dll is loaded at run
    # time, so a system without it keeps the colour tint instead
    user32 comctl32 d3d11 d2d1 dwrite dxguid
  )
endif()
