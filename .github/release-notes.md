## Install

**macOS 11 or later** (Apple silicon and Intel): download `ReaMesh_{{VERSION}}.dmg`, open it and drag ReaMesh into Applications. ReaMesh is not notarized by Apple, so macOS blocks the first launch: open System Settings → Privacy & Security, click "Open Anyway" next to the ReaMesh message and confirm. This is needed once; later updates install from inside the app.

**Windows (64-bit)**: run `ReaMesh_{{VERSION}}_x64-setup.exe`. The installer is not code-signed, so SmartScreen may show "Windows protected your PC": click "More info", then "Run anyway".

On first launch, click Install in the app to add the REAPER extension (REAPER 7.12 or later), then restart REAPER.

**Relay** (optional, for sessions over the internet): `reamesh-relay-linux-x86_64` and `-aarch64`, or the image `ghcr.io/reamesh/reamesh-relay:{{VERSION}}`. Running relays update themselves; see [the relay's README](https://github.com/ReaMesh/ReaMesh/tree/main/relay).
