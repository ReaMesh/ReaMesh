#ifndef REAMESH_BLE_H
#define REAMESH_BLE_H

#include <stdint.h>

// Nearby-session hints over Bluetooth LE (CoreBluetooth), for networks where
// LAN discovery is blocked. A host advertises one ReaMesh service; its single
// readable characteristic holds a short JSON payload with the host's dialable
// IPs and ports (no name, no peer id: those arrive later, signed, over the
// network connection). A scanner reads it and hands it to Rust, which passes
// it on to the extension as a dial hint. Nothing here decides what is shown.
//
// The managers are created on first use, which is when macOS asks for the
// Bluetooth permission: Rust only calls advertise/scan once the user turned
// the feature on.

// CBManager authorization, plus "unsupported".
enum {
    kReaMeshBleUnsupported = -1,
    kReaMeshBleNotDetermined = 0,
    kReaMeshBleRestricted = 1,
    kReaMeshBleDenied = 2,
    kReaMeshBleAllowed = 3,
};

// Called on a private queue with a payload read from a nearby host (UTF-8,
// at most 512 bytes, not validated).
typedef void (*ReaMeshBleFound)(const char *payload);

void reamesh_ble_init(ReaMeshBleFound found);
int32_t reamesh_ble_authorization(void);
// Advertise `payload` (UTF-8, at most 512 bytes); NULL or "" stops.
void reamesh_ble_advertise(const char *payload);
// 1 = scan for nearby hosts, 0 = stop.
void reamesh_ble_scan(int32_t on);

#endif
