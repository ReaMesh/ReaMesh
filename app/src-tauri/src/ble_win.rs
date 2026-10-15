//! Windows side of nearby sessions (see nearby.rs), on the WinRT Bluetooth
//! APIs. It mirrors src/ble.m: a host publishes one GATT service with one
//! readable characteristic answered from the current payload and advertises
//! only the service UUID; a scanner watches for that UUID, connects, reads the
//! characteristic and hands the text to `on_found`.
//!
//! Every WinRT object lives on one worker thread, fed by a channel; the Tauri
//! commands only post to it and read atomics, so nothing blocks the main
//! thread. Reads run on short-lived threads of their own (at most
//! `MAX_CONNECTING`), each operation bounded by the read's deadline.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{channel, sync_channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{RuntimeType, GUID};
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
    BluetoothLEAdvertisementWatcherStatus, BluetoothLEScanningMode,
};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicProperties, GattCommunicationStatus, GattDeviceService,
    GattLocalCharacteristic, GattLocalCharacteristicParameters, GattProtectionLevel,
    GattProtocolError, GattReadRequest, GattReadRequestedEventArgs, GattServiceProvider,
    GattServiceProviderAdvertisementStatus, GattServiceProviderAdvertisingParameters,
};
use windows::Devices::Bluetooth::{
    BluetoothAdapter, BluetoothAddressType, BluetoothCacheMode, BluetoothError, BluetoothLEDevice,
};
use windows::Devices::Radios::{Radio, RadioAccessStatus};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, DataWriter};
use windows_future::IAsyncOperation;

const SERVICE: GUID = GUID::from_u128(0xFC403C43_CEB1_43BD_9890_FD4198392FDD);
const PAYLOAD: GUID = GUID::from_u128(0x176F36B1_74E7_4B2F_A3C4_24620E09B8CD);
const MAX_PAYLOAD: usize = 512;
// The same host is read again at most this often (its ports change when it
// re-hosts), and a read that hangs is given up after CONNECT_TIMEOUT.
const REREAD_INTERVAL: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
// Advertisements are readable by anyone nearby: bound the work one can cause.
const MAX_CONNECTING: usize = 3;
const MAX_REMEMBERED: usize = 256;
/// How often the worker re-checks what it runs: Bluetooth switched off stops
/// the watcher and the advertisement, and they come back on this tick.
const TICK: Duration = Duration::from_secs(2);
/// How often the adapter is looked for again (a dongle plugged in later).
const PROBE_INTERVAL: Duration = Duration::from_secs(30);
/// Bound for the setup calls (adapter lookup, access, publishing the service).
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// An adapter with the LE central and peripheral roles is present.
static SUPPORTED: AtomicBool = AtomicBool::new(false);
/// Radio access as nearby.rs codes it (0 undetermined … 3 allowed); asked
/// for only once the feature is in use, like the macOS permission.
static AUTH: AtomicI32 = AtomicI32::new(0);
static TX: OnceLock<Sender<Msg>> = OnceLock::new();

enum Msg {
    Scan(bool),
    Advertise(Option<Vec<u8>>),
    Seen(u64, BluetoothAddressType),
    Read(u64, Option<String>),
}

pub fn init() {
    let (tx, rx) = channel();
    if TX.set(tx.clone()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("reamesh-ble".into()).spawn(move || {
        let mut w = Worker {
            tx,
            want_scan: false,
            watcher: None,
            payload: Arc::new(Mutex::new(None)),
            provider: None,
            last_read: HashMap::new(),
            connecting: HashSet::new(),
            probed: None,
            access_checked: None,
        };
        loop {
            match rx.recv_timeout(TICK) {
                Ok(msg) => w.handle(msg),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return,
            }
            w.reconcile();
        }
    });
}

pub fn supported() -> bool {
    SUPPORTED.load(Ordering::Relaxed)
}

pub fn authorization() -> i32 {
    if supported() {
        AUTH.load(Ordering::Relaxed)
    } else {
        -1
    }
}

pub fn advertise(payload: Option<&str>) {
    let data = payload.map(|p| p.as_bytes().to_vec()).filter(|d| !d.is_empty() && d.len() <= MAX_PAYLOAD);
    send(Msg::Advertise(data));
}

pub fn scan(on: bool) {
    send(Msg::Scan(on));
}

fn send(msg: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(msg);
    }
}

struct Worker {
    tx: Sender<Msg>,
    // Scanner side.
    want_scan: bool,
    watcher: Option<BluetoothLEAdvertisementWatcher>,
    last_read: HashMap<u64, Instant>,
    connecting: HashSet<u64>,
    // Host side. Reads are answered from `payload`, so a re-host's new ports
    // are served without publishing the service again.
    payload: Arc<Mutex<Option<Vec<u8>>>>,
    provider: Option<GattServiceProvider>,
    probed: Option<Instant>,
    access_checked: Option<Instant>,
}

impl Worker {
    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Scan(on) => self.want_scan = on,
            Msg::Advertise(data) => *self.payload.lock().unwrap_or_else(|e| e.into_inner()) = data,
            Msg::Seen(addr, kind) => self.seen(addr, kind),
            Msg::Read(addr, value) => {
                self.connecting.remove(&addr);
                if let (true, Some(v)) = (self.want_scan, value) {
                    super::on_found(&v);
                }
            }
        }
    }

    fn advertising(&self) -> bool {
        self.payload.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    /// Bring the watcher and the advertisement in line with what is wanted.
    fn reconcile(&mut self) {
        if self.probed.is_none_or(|t| t.elapsed() >= PROBE_INTERVAL) {
            self.probed = Some(Instant::now());
            let ok = probe();
            if SUPPORTED.swap(ok, Ordering::Relaxed) != ok {
                super::changed();
            }
        }
        let wanted = self.want_scan || self.advertising();
        if supported() && wanted && self.access_checked.is_none_or(|t| {
            AUTH.load(Ordering::Relaxed) != 3 && t.elapsed() >= Duration::from_secs(5)
        }) {
            self.access_checked = Some(Instant::now());
            let a = access();
            if AUTH.swap(a, Ordering::Relaxed) != a {
                super::changed();
            }
        }
        // Denied: out of the app's hands (undetermined still tries).
        let usable = supported() && matches!(AUTH.load(Ordering::Relaxed), 0 | 3);

        if usable && self.want_scan {
            let running = self.watcher.as_ref().and_then(|w| w.Status().ok()).is_some_and(|s| {
                s != BluetoothLEAdvertisementWatcherStatus::Stopped
                    && s != BluetoothLEAdvertisementWatcherStatus::Aborted
            });
            if !running {
                self.stop_watcher();
                self.watcher = self.start_watcher();
            }
        } else {
            self.stop_watcher();
        }

        if usable && self.advertising() {
            let running = self.provider.as_ref().and_then(|p| p.AdvertisementStatus().ok()).is_some_and(|s| {
                s != GattServiceProviderAdvertisementStatus::Stopped
                    && s != GattServiceProviderAdvertisementStatus::Aborted
            });
            if !running {
                self.stop_provider();
                self.provider = self.start_provider();
            }
        } else {
            self.stop_provider();
        }
    }

    fn start_watcher(&self) -> Option<BluetoothLEAdvertisementWatcher> {
        let w = BluetoothLEAdvertisementWatcher::new().ok()?;
        w.SetScanningMode(BluetoothLEScanningMode::Active).ok()?;
        w.AdvertisementFilter().ok()?.Advertisement().ok()?.ServiceUuids().ok()?.Append(SERVICE).ok()?;
        let tx = self.tx.clone();
        let handler = TypedEventHandler::<BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementReceivedEventArgs>::new(
            move |_, args| {
                if let Some(a) = args.as_ref() {
                    let _ = tx.send(Msg::Seen(a.BluetoothAddress()?, a.BluetoothAddressType()?));
                }
                Ok(())
            },
        );
        w.Received(&handler).ok()?;
        w.Start().ok()?;
        Some(w)
    }

    fn stop_watcher(&mut self) {
        if let Some(w) = self.watcher.take() {
            let _ = w.Stop();
        }
    }

    fn start_provider(&self) -> Option<GattServiceProvider> {
        let res = wait(GattServiceProvider::CreateAsync(SERVICE).ok()?, Instant::now() + SETUP_TIMEOUT)?;
        if res.Error().ok()? != BluetoothError::Success {
            return None;
        }
        let provider = res.ServiceProvider().ok()?;
        let params = GattLocalCharacteristicParameters::new().ok()?;
        params.SetCharacteristicProperties(GattCharacteristicProperties::Read).ok()?;
        params.SetReadProtectionLevel(GattProtectionLevel::Plain).ok()?;
        let made = wait(
            provider.Service().ok()?.CreateCharacteristicAsync(PAYLOAD, &params).ok()?,
            Instant::now() + SETUP_TIMEOUT,
        )?;
        if made.Error().ok()? != BluetoothError::Success {
            return None;
        }
        let payload = self.payload.clone();
        let handler = TypedEventHandler::<GattLocalCharacteristic, GattReadRequestedEventArgs>::new(move |_, args| {
            let Some(args) = args.as_ref() else { return Ok(()) };
            let deferral = args.GetDeferral()?;
            let payload = payload.clone();
            args.GetRequestAsync()?.when(move |req| {
                if let Ok(req) = req {
                    let data = payload.lock().ok().and_then(|p| p.clone());
                    respond(&req, data.as_deref());
                }
                let _ = deferral.Complete();
            })
        });
        made.Characteristic().ok()?.ReadRequested(&handler).ok()?;
        // Only the service UUID: Windows adds no local name to it.
        let adv = GattServiceProviderAdvertisingParameters::new().ok()?;
        adv.SetIsConnectable(true).ok()?;
        adv.SetIsDiscoverable(true).ok()?;
        provider.StartAdvertisingWithParameters(&adv).ok()?;
        Some(provider)
    }

    fn stop_provider(&mut self) {
        if let Some(p) = self.provider.take() {
            let _ = p.StopAdvertising();
        }
    }

    fn seen(&mut self, addr: u64, kind: BluetoothAddressType) {
        if !self.want_scan || self.connecting.contains(&addr) || self.connecting.len() >= MAX_CONNECTING {
            return;
        }
        if self.last_read.get(&addr).is_some_and(|t| t.elapsed() < REREAD_INTERVAL) {
            return;
        }
        if self.last_read.len() >= MAX_REMEMBERED {
            self.last_read.retain(|_, t| t.elapsed() < REREAD_INTERVAL);
            if self.last_read.len() >= MAX_REMEMBERED {
                self.last_read.clear();
            }
        }
        // Stamped before the attempt, so a host that fails to answer is
        // retried on the same schedule instead of on every advertisement.
        self.last_read.insert(addr, Instant::now());
        self.connecting.insert(addr);
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new().name("reamesh-ble-read".into()).spawn(move || {
            let _ = tx.send(Msg::Read(addr, read_payload(addr, kind)));
        });
        if spawned.is_err() {
            self.connecting.remove(&addr);
        }
    }
}

/// Answer a read of the payload characteristic (long reads come with an
/// offset).
fn respond(req: &GattReadRequest, data: Option<&[u8]>) {
    let Some(data) = data else {
        let _ = req.RespondWithProtocolError(GattProtocolError::AttributeNotFound().unwrap_or(0x0A));
        return;
    };
    let offset = req.Offset().unwrap_or(0) as usize;
    if offset > data.len() {
        let _ = req.RespondWithProtocolError(GattProtocolError::InvalidOffset().unwrap_or(0x07));
        return;
    }
    let Ok(w) = DataWriter::new() else { return };
    if w.WriteBytes(&data[offset..]).is_ok() {
        if let Ok(buf) = w.DetachBuffer() {
            let _ = req.RespondWithValue(&buf);
        }
    }
}

/// Connect to a host that advertised the service and read its payload, all
/// within CONNECT_TIMEOUT.
fn read_payload(addr: u64, kind: BluetoothAddressType) -> Option<String> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let op = BluetoothLEDevice::FromBluetoothAddressWithBluetoothAddressTypeAsync(addr, kind).ok()?;
    let dev = wait(op, deadline)?;
    let out = read_device(&dev, deadline);
    // Closing every handle is what drops the connection.
    let _ = dev.Close();
    out
}

fn read_device(dev: &BluetoothLEDevice, deadline: Instant) -> Option<String> {
    // Uncached: a host that re-hosted published its service anew.
    let op = dev.GetGattServicesForUuidWithCacheModeAsync(SERVICE, BluetoothCacheMode::Uncached).ok()?;
    let res = wait(op, deadline)?;
    if res.Status().ok()? != GattCommunicationStatus::Success {
        return None;
    }
    let services = res.Services().ok()?;
    let mut out = None;
    for i in 0..services.Size().ok()? {
        let Ok(s) = services.GetAt(i) else { continue };
        if out.is_none() {
            out = read_service(&s, deadline);
        }
        let _ = s.Close();
    }
    out
}

fn read_service(s: &GattDeviceService, deadline: Instant) -> Option<String> {
    let op = s.GetCharacteristicsForUuidWithCacheModeAsync(PAYLOAD, BluetoothCacheMode::Uncached).ok()?;
    let res = wait(op, deadline)?;
    if res.Status().ok()? != GattCommunicationStatus::Success {
        return None;
    }
    let chars = res.Characteristics().ok()?;
    if chars.Size().ok()? == 0 {
        return None;
    }
    let c = chars.GetAt(0).ok()?;
    let read = wait(c.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached).ok()?, deadline)?;
    if read.Status().ok()? != GattCommunicationStatus::Success {
        return None;
    }
    let buf = read.Value().ok()?;
    let n = buf.Length().ok()? as usize;
    if n == 0 || n > MAX_PAYLOAD {
        return None;
    }
    let mut v = vec![0u8; n];
    DataReader::FromBuffer(&buf).ok()?.ReadBytes(&mut v).ok()?;
    String::from_utf8(v).ok()
}

/// An adapter that can both scan and advertise.
fn probe() -> bool {
    let Some(a) = BluetoothAdapter::GetDefaultAsync().ok().and_then(|op| wait(op, Instant::now() + SETUP_TIMEOUT))
    else {
        return false;
    };
    a.IsLowEnergySupported().unwrap_or(false)
        && a.IsPeripheralRoleSupported().unwrap_or(false)
        && a.IsCentralRoleSupported().unwrap_or(false)
}

/// Radio access, coded as nearby.rs reads it: whether Windows lets this app
/// use the Bluetooth radio.
fn access() -> i32 {
    match Radio::RequestAccessAsync().ok().and_then(|op| wait(op, Instant::now() + SETUP_TIMEOUT)) {
        Some(RadioAccessStatus::Allowed) => 3,
        Some(RadioAccessStatus::DeniedByUser) => 2,
        Some(RadioAccessStatus::DeniedBySystem) => 1,
        _ => 0,
    }
}

/// Wait for a WinRT operation until `deadline`; cancel it past that.
fn wait<T: RuntimeType + Send + 'static>(op: IAsyncOperation<T>, deadline: Instant) -> Option<T> {
    let (tx, rx) = sync_channel(1);
    op.when(move |r| {
        let _ = tx.send(r);
    })
    .ok()?;
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(r) => r.ok(),
        Err(_) => {
            let _ = op.Cancel();
            None
        }
    }
}
