//! Linux side of nearby sessions (see nearby.rs), on BlueZ over D-Bus. It
//! mirrors src/ble.m and src/ble_win.rs: a host registers one GATT service
//! with one readable characteristic answered from the current payload and
//! advertises only the service UUID; a scanner runs a discovery filtered on
//! that UUID, connects to what it finds, reads the characteristic and hands
//! the text to `on_found`.
//!
//! One worker thread, fed by a channel, owns the bus connection and everything
//! registered over it; the Tauri commands only post to it and read atomics, so
//! nothing blocks the main thread. Reads run on short-lived threads of their
//! own (at most `MAX_CONNECTING`). Every D-Bus exchange is bounded by a
//! deadline (`within`): bluetoothd may hang and a host may never answer.
//!
//! bluetoothd drops a client's discovery, advertisement and GATT application
//! when the client leaves the bus, so quitting the app needs no teardown.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Poll;
use std::time::{Duration, Instant};

use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{interface, Connection};

// BlueZ writes UUIDs in lower case.
const SERVICE: &str = "fc403c43-ceb1-43bd-9890-fd4198392fdd";
const PAYLOAD: &str = "176f36b1-74e7-4b2f-a3c4-24620e09b8cd";
const MAX_PAYLOAD: usize = 512;
// The same host is read again at most this often (its ports change when it
// re-hosts), and a read that hangs is given up after CONNECT_TIMEOUT.
const REREAD_INTERVAL: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
// Advertisements are readable by anyone nearby: bound the work one can cause.
const MAX_CONNECTING: usize = 3;
const MAX_REMEMBERED: usize = 256;
/// How often the worker looks at BlueZ while it has something to run: the
/// hosts a discovery found, and whether the adapter, its power and bluetoothd
/// itself are still what the registrations were made against.
const TICK: Duration = Duration::from_secs(2);
/// How often BlueZ and its adapter are looked for while there is nothing to
/// run or nothing to run it on (a dongle plugged in later, bluetoothd started
/// later).
const PROBE_INTERVAL: Duration = Duration::from_secs(30);
/// Bound for the setup calls (reaching the bus, registering, cleaning up).
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
/// A discovery or advertisement bluetoothd refused is tried again this much
/// later, not on every tick.
const RETRY_INTERVAL: Duration = Duration::from_secs(10);
/// How often a connected host is asked whether its services are known yet.
const RESOLVE_POLL: Duration = Duration::from_millis(200);

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const ADV_MANAGER: &str = "org.bluez.LEAdvertisingManager1";
const GATT_MANAGER: &str = "org.bluez.GattManager1";
const GATT_SERVICE: &str = "org.bluez.GattService1";
const GATT_CHARACTERISTIC: &str = "org.bluez.GattCharacteristic1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

// Where this app's objects live on its own bus connection. The advertisement
// sits outside the GATT application's tree, which bluetoothd reads as a whole.
const APP_PATH: &str = "/com/reamesh/nearby";
const SERVICE_PATH: &str = "/com/reamesh/nearby/service0";
const CHARACTERISTIC_PATH: &str = "/com/reamesh/nearby/service0/char0";
const ADV_PATH: &str = "/com/reamesh/advertisement";

type Props = HashMap<String, OwnedValue>;
/// What `GetManagedObjects` returns: path → interface → properties.
type Objects = HashMap<OwnedObjectPath, HashMap<String, Props>>;

/// BlueZ is on the system bus with an adapter that can scan, advertise and
/// serve GATT.
static SUPPORTED: AtomicBool = AtomicBool::new(false);
/// Access as nearby.rs codes it: 0 until BlueZ took a call, 3 once it did, 1
/// (restricted, as Windows reports a system-wide refusal) when the system bus
/// policy or bluetoothd refused this user: that is the administrator's call,
/// not one the user made in a prompt. A radio that is switched off or
/// rfkill-blocked is no refusal: like on the other platforms nothing runs and
/// nothing is said until it is back.
static AUTH: AtomicI32 = AtomicI32::new(0);
const AUTH_REFUSED: i32 = 1;
static TX: OnceLock<Sender<Msg>> = OnceLock::new();

enum Msg {
    Scan(bool),
    Advertise(Option<Vec<u8>>),
    /// A read ended: the device's object path and what it held.
    Read(String, Option<String>),
    /// bluetoothd dropped the advertisement on its own.
    Released,
}

pub fn init() {
    let (tx, rx) = channel();
    if TX.set(tx.clone()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("reamesh-ble".into()).spawn(move || {
        let mut w = Worker::new(tx, None);
        w.reconcile();
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

fn set_auth(a: i32) {
    if AUTH.swap(a, Ordering::Relaxed) != a {
        super::changed();
    }
}

struct Worker {
    tx: Sender<Msg>,
    /// The bus to use instead of the system bus (tests run a private one).
    address: Option<String>,
    conn: Option<Connection>,
    /// bluetoothd's name on the bus: a restarted daemon has another one and
    /// remembers nothing this app registered.
    bluez: Option<String>,
    /// The adapter in use.
    adapter: Option<String>,
    probed: Option<Instant>,
    denied: Option<Instant>,
    // Scanner side.
    want_scan: bool,
    /// The adapter a discovery of ours runs on.
    scanning: Option<String>,
    scan_failed: Option<Instant>,
    scan_asserted: Option<Instant>,
    last_read: HashMap<String, Instant>,
    connecting: HashSet<String>,
    /// The device objects BlueZ had at the last look, and those among them
    /// that appeared while a discovery of ours ran: only those may be
    /// removed again after a read, never one that was there before.
    devices: HashSet<String>,
    created: HashSet<String>,
    // Host side. Reads are answered from `payload`, so a re-host's new ports
    // are served without registering the service again.
    payload: Arc<Mutex<Option<Vec<u8>>>>,
    /// The adapter the service and its advertisement are registered on.
    advertised: Option<String>,
    adv_failed: Option<Instant>,
}

impl Worker {
    fn new(tx: Sender<Msg>, address: Option<String>) -> Self {
        Worker {
            tx,
            address,
            conn: None,
            bluez: None,
            adapter: None,
            probed: None,
            denied: None,
            want_scan: false,
            scanning: None,
            scan_failed: None,
            scan_asserted: None,
            last_read: HashMap::new(),
            connecting: HashSet::new(),
            devices: HashSet::new(),
            created: HashSet::new(),
            payload: Arc::new(Mutex::new(None)),
            advertised: None,
            adv_failed: None,
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Scan(on) => self.want_scan = on,
            Msg::Advertise(data) => *self.payload.lock().unwrap_or_else(|e| e.into_inner()) = data,
            Msg::Read(dev, value) => {
                self.connecting.remove(&dev);
                if let (true, Some(v)) = (self.want_scan, value) {
                    super::on_found(&v);
                }
            }
            // The service is still registered: take it down as well, so the
            // next pass starts both from nothing.
            Msg::Released => self.stop_advertising(),
        }
    }

    fn hosting(&self) -> bool {
        self.payload.lock().map(|p| p.is_some()).unwrap_or(false)
    }

    /// Bring the discovery and the advertisement in line with what is wanted
    /// and with what BlueZ looks like right now.
    fn reconcile(&mut self) {
        let wanted = self.want_scan || self.hosting();
        let running = self.scanning.is_some() || self.advertised.is_some();
        let due = self.probed.is_none_or(|t| t.elapsed() >= PROBE_INTERVAL);
        if !due && !(supported() && (wanted || running)) {
            return;
        }
        self.probed = Some(Instant::now());
        let seen = self.observe();
        let owner_seen = seen.is_some();
        let (owner, objects) = match seen {
            Some((owner, objects)) => (Some(owner), objects),
            None => (None, Objects::new()),
        };
        // bluetoothd went away or came back as another process: whatever was
        // registered died with the old one.
        if owner != self.bluez {
            self.bluez = owner;
            self.forget();
        }
        if owner_seen {
            self.note_devices(&objects);
        }
        let adapters = adapters(&objects);
        let adapter = pick(&adapters, self.adapter.as_deref());
        let ok = adapter.is_some();
        if SUPPORTED.swap(ok, Ordering::Relaxed) != ok {
            super::changed();
        }
        let Some(adapter) = adapter else {
            // Unplugged: its registrations went with it.
            self.adapter = None;
            self.forget();
            return;
        };
        if self.adapter.as_deref() != Some(adapter.path.as_str()) {
            self.stop_scanning();
            self.stop_advertising();
            self.adapter = Some(adapter.path.clone());
        }
        // Switched off is the user's call and stays that way; refused is
        // asked again now and then, in case the policy changed.
        let refused = AUTH.load(Ordering::Relaxed) == AUTH_REFUSED
            && self.denied.is_some_and(|t| t.elapsed() < PROBE_INTERVAL);
        let usable = adapter.powered && !refused;

        if usable && self.want_scan {
            if self.scanning.is_none() {
                if self.scan_failed.is_none_or(|t| t.elapsed() >= RETRY_INTERVAL) {
                    self.start_scanning(&adapter.path);
                }
            } else if !adapter.discovering && self.scan_asserted.is_none_or(|t| t.elapsed() >= RETRY_INTERVAL) {
                // The adapter reads idle between two rounds of a discovery,
                // and for good after a power cycle that ended ours: asking
                // again tells the two apart and restarts the second.
                self.start_scanning(&adapter.path);
            }
            if self.scanning.is_some() {
                for dev in hosts(&objects, &adapter.path) {
                    self.seen(&adapter.path, dev);
                }
            }
        } else {
            self.stop_scanning();
        }

        if usable && self.hosting() {
            // Zero advertisements on the adapter means ours is gone too (a
            // power cycle between two ticks).
            if self.advertised.is_some() && adapter.advertisements == Some(0) {
                self.stop_advertising();
            }
            if self.advertised.is_none() && self.adv_failed.is_none_or(|t| t.elapsed() >= RETRY_INTERVAL) {
                self.start_advertising(&adapter.path);
            }
        } else {
            self.stop_advertising();
        }
    }

    /// bluetoothd's bus name and everything it exports, or `None` without a
    /// bus, without BlueZ, or when either stopped answering.
    fn observe(&mut self) -> Option<(String, Objects)> {
        if self.conn.is_none() {
            self.conn = connect(self.address.as_deref());
        }
        let conn = self.conn.clone()?;
        match within(Instant::now() + SETUP_TIMEOUT, snapshot(&conn)) {
            Some(Ok(seen)) => seen,
            _ => {
                // A fresh connection at the next probe; leaving the bus also
                // makes bluetoothd drop what this one had registered.
                self.conn = None;
                None
            }
        }
    }

    /// Keep track of which device objects a discovery of ours brought in.
    /// Another program's discovery running at the same time may add to them;
    /// those are passers-by BlueZ would drop on its own as well.
    fn note_devices(&mut self, objects: &Objects) {
        let now: HashSet<String> =
            objects.iter().filter(|(_, i)| i.contains_key(DEVICE)).map(|(p, _)| p.to_string()).collect();
        if self.scanning.is_some() {
            self.created.extend(now.difference(&self.devices).cloned());
        }
        self.created.retain(|d| now.contains(d));
        self.devices = now;
    }

    /// Nothing is registered any more, and there is nobody to tell.
    fn forget(&mut self) {
        self.scanning = None;
        self.advertised = None;
        self.scan_failed = None;
        self.adv_failed = None;
        self.denied = None;
    }

    /// Whether a call to bluetoothd went through, noting a refusal.
    fn accepted(&mut self, done: Option<zbus::Result<()>>) -> bool {
        match done {
            Some(Ok(())) => {
                self.denied = None;
                set_auth(3);
                true
            }
            Some(Err(e)) if is_refusal(&e) => {
                self.denied = Some(Instant::now());
                set_auth(AUTH_REFUSED);
                false
            }
            _ => false,
        }
    }

    fn start_scanning(&mut self, adapter: &str) {
        let Some(conn) = self.conn.clone() else { return };
        self.scan_asserted = Some(Instant::now());
        let done = within(Instant::now() + SETUP_TIMEOUT, async {
            // Only LE, and only devices advertising the service. Filters of
            // other programs are merged into the scan, so every device is
            // still checked against the UUID (`hosts`).
            let filter: HashMap<&str, Value> =
                HashMap::from([("UUIDs", Value::from(vec![SERVICE])), ("Transport", Value::from("le"))]);
            call(&conn, adapter, ADAPTER, "SetDiscoveryFilter", &filter).await?;
            match call(&conn, adapter, ADAPTER, "StartDiscovery", &()).await {
                // Ours is still running.
                Err(e) if is_error(&e, "org.bluez.Error.InProgress") => Ok(()),
                r => r.map(drop),
            }
        });
        if self.accepted(done) {
            self.scanning = Some(adapter.to_string());
            self.scan_failed = None;
        } else {
            // Also when a running discovery could not be confirmed: it is
            // ended and started over.
            self.stop_scanning();
            self.scan_failed = Some(Instant::now());
        }
    }

    /// Ends this app's discovery; one another program runs goes on. Reads
    /// under way finish on their own and disconnect.
    fn stop_scanning(&mut self) {
        let Some(adapter) = self.scanning.take() else { return };
        let Some(conn) = self.conn.clone() else { return };
        let _ = within(Instant::now() + SETUP_TIMEOUT, call(&conn, &adapter, ADAPTER, "StopDiscovery", &()));
    }

    fn start_advertising(&mut self, adapter: &str) {
        let Some(conn) = self.conn.clone() else { return };
        let (payload, tx) = (self.payload.clone(), self.tx.clone());
        let done = within(Instant::now() + SETUP_TIMEOUT, async {
            let server = conn.object_server();
            server.at(SERVICE_PATH, Service).await?;
            server.at(CHARACTERISTIC_PATH, Characteristic { payload }).await?;
            server.at(APP_PATH, zbus::fdo::ObjectManager).await?;
            server.at(ADV_PATH, Advertisement { tx }).await?;
            let none = HashMap::<&str, Value>::new();
            // The service first, so it is there when the first guest connects.
            call(&conn, adapter, GATT_MANAGER, "RegisterApplication", &(object(APP_PATH), &none)).await?;
            call(&conn, adapter, ADV_MANAGER, "RegisterAdvertisement", &(object(ADV_PATH), &none)).await?;
            Ok(())
        });
        // Set either way, so a half-made registration is taken down again.
        self.advertised = Some(adapter.to_string());
        if self.accepted(done) {
            self.adv_failed = None;
        } else {
            self.stop_advertising();
            self.adv_failed = Some(Instant::now());
        }
    }

    fn stop_advertising(&mut self) {
        let Some(adapter) = self.advertised.take() else { return };
        let Some(conn) = self.conn.clone() else { return };
        let _ = within(Instant::now() + SETUP_TIMEOUT, async {
            let _ = call(&conn, &adapter, ADV_MANAGER, "UnregisterAdvertisement", &object(ADV_PATH)).await;
            let _ = call(&conn, &adapter, GATT_MANAGER, "UnregisterApplication", &object(APP_PATH)).await;
            // Nothing answers for the payload while this machine hosts nothing.
            let server = conn.object_server();
            let _ = server.remove::<Advertisement, _>(ADV_PATH).await;
            let _ = server.remove::<zbus::fdo::ObjectManager, _>(APP_PATH).await;
            let _ = server.remove::<Characteristic, _>(CHARACTERISTIC_PATH).await;
            let _ = server.remove::<Service, _>(SERVICE_PATH).await;
        });
    }

    /// A host is in range: read it, unless it was read a moment ago or enough
    /// reads are under way.
    fn seen(&mut self, adapter: &str, dev: String) {
        if self.connecting.contains(&dev) || self.connecting.len() >= MAX_CONNECTING {
            return;
        }
        if self.last_read.get(&dev).is_some_and(|t| t.elapsed() < REREAD_INTERVAL) {
            return;
        }
        if self.last_read.len() >= MAX_REMEMBERED {
            self.last_read.retain(|_, t| t.elapsed() < REREAD_INTERVAL);
            if self.last_read.len() >= MAX_REMEMBERED {
                self.last_read.clear();
            }
        }
        let Some(conn) = self.conn.clone() else { return };
        // Stamped before the attempt, so a host that fails to answer is
        // retried on the same schedule instead of on every tick.
        self.last_read.insert(dev.clone(), Instant::now());
        self.connecting.insert(dev.clone());
        let (tx, adapter, path) = (self.tx.clone(), adapter.to_string(), dev.clone());
        let ours = self.created.contains(&dev);
        let spawned = std::thread::Builder::new().name("reamesh-ble-read".into()).spawn(move || {
            let value = read_payload(&conn, &adapter, &path, ours);
            let _ = tx.send(Msg::Read(path, value));
        });
        if spawned.is_err() {
            self.connecting.remove(&dev);
        }
    }
}

// ── The host's objects, as bluetoothd reads them ───────────────────────────

struct Service;

#[interface(name = "org.bluez.GattService1")]
impl Service {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        SERVICE.into()
    }

    #[zbus(property)]
    fn primary(&self) -> bool {
        true
    }
}

struct Characteristic {
    payload: Arc<Mutex<Option<Vec<u8>>>>,
}

#[interface(name = "org.bluez.GattCharacteristic1")]
impl Characteristic {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        PAYLOAD.into()
    }

    #[zbus(property)]
    fn service(&self) -> OwnedObjectPath {
        object(SERVICE_PATH).into()
    }

    /// Readable without pairing, and nothing else.
    #[zbus(property)]
    fn flags(&self) -> Vec<String> {
        vec!["read".into()]
    }

    /// A guest reads the payload; bluetoothd passes the offset of a long read.
    fn read_value(&self, options: HashMap<String, OwnedValue>) -> Result<Vec<u8>, GattError> {
        let offset = match options.get("offset").map(|v| &**v) {
            Some(Value::U16(o)) => usize::from(*o),
            _ => 0,
        };
        let data = self.payload.lock().ok().and_then(|p| p.clone());
        let Some(data) = data else {
            return Err(GattError::Failed("not hosting".into()));
        };
        from_offset(&data, offset).map(<[u8]>::to_vec).ok_or_else(|| GattError::InvalidOffset("past the end".into()))
    }
}

/// The rest of the value from `offset` on; `None` past its end (reading from
/// exactly the end is how a long read finishes).
fn from_offset(data: &[u8], offset: usize) -> Option<&[u8]> {
    data.get(offset..)
}

/// The error names bluetoothd turns into ATT errors.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum GattError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    InvalidOffset(String),
}

struct Advertisement {
    tx: Sender<Msg>,
}

/// Only the service UUID: no `LocalName` and no `Includes`, so the
/// advertisement doesn't carry the computer's name either.
#[interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    /// bluetoothd removed the advertisement (it is already unregistered).
    fn release(&self) {
        let _ = self.tx.send(Msg::Released);
    }

    /// Connectable.
    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "peripheral".into()
    }

    #[zbus(property, name = "ServiceUUIDs")]
    fn service_uuids(&self) -> Vec<String> {
        vec![SERVICE.into()]
    }

    /// Flagged general-discoverable whatever the adapter's own setting is: a
    /// BlueZ scanner skips advertisements without the flag.
    #[zbus(property)]
    fn discoverable(&self) -> bool {
        true
    }
}

// ── Talking to bluetoothd ──────────────────────────────────────────────────

/// Runs `fut` on this thread until it is done or `deadline` has passed.
fn within<T>(deadline: Instant, fut: impl Future<Output = T>) -> Option<T> {
    async_io::block_on(async {
        let mut fut = std::pin::pin!(fut);
        let mut timer = std::pin::pin!(async_io::Timer::at(deadline));
        std::future::poll_fn(|cx| {
            if let Poll::Ready(v) = fut.as_mut().poll(cx) {
                return Poll::Ready(Some(v));
            }
            timer.as_mut().poll(cx).map(|_| None)
        })
        .await
    })
}

fn connect(address: Option<&str>) -> Option<Connection> {
    within(Instant::now() + SETUP_TIMEOUT, async {
        let builder = match address {
            Some(a) => zbus::connection::Builder::address(a),
            None => zbus::connection::Builder::system(),
        };
        builder.ok()?.build().await.ok()
    })?
}

fn object(path: &str) -> ObjectPath<'_> {
    ObjectPath::from_str_unchecked(path)
}

async fn call<B>(conn: &Connection, path: &str, interface: &str, method: &str, body: &B) -> zbus::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(BLUEZ), path, Some(interface), method, body).await
}

fn is_error(e: &zbus::Error, name: &str) -> bool {
    matches!(e, zbus::Error::MethodError(n, _, _) if n.as_str() == name)
}

/// The bus policy keeps this user away from BlueZ, or bluetoothd itself said
/// no.
fn is_refusal(e: &zbus::Error) -> bool {
    is_error(e, "org.freedesktop.DBus.Error.AccessDenied") || is_error(e, "org.bluez.Error.NotAuthorized")
}

/// bluetoothd's unique bus name and its objects; `None` when it isn't running.
async fn snapshot(conn: &Connection) -> zbus::Result<Option<(String, Objects)>> {
    // Asked of the bus, not of BlueZ: a call to `org.bluez` would start a
    // bluetoothd that is installed but was left off.
    let running: bool = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &BLUEZ,
        )
        .await?
        .body()
        .deserialize()?;
    if !running {
        return Ok(None);
    }
    let reply = call(conn, "/", OBJECT_MANAGER, "GetManagedObjects", &()).await?;
    let owner = reply.header().sender().map(|s| s.to_string()).unwrap_or_default();
    let objects = reply.body().deserialize()?;
    Ok(Some((owner, objects)))
}

async fn properties(conn: &Connection, path: &str, interface: &str) -> zbus::Result<Props> {
    call(conn, path, PROPERTIES, "GetAll", &interface).await?.body().deserialize()
}

fn flag(p: &Props, key: &str) -> bool {
    matches!(p.get(key).map(|v| &**v), Some(Value::Bool(true)))
}

fn text<'a>(p: &'a Props, key: &str) -> Option<&'a str> {
    match &**p.get(key)? {
        Value::Str(s) => Some(s.as_str()),
        Value::ObjectPath(o) => Some(o.as_str()),
        _ => None,
    }
}

fn list<'a>(p: &'a Props, key: &str) -> Vec<&'a str> {
    match p.get(key).map(|v| &**v) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn has_uuid(p: &Props, key: &str, uuid: &str) -> bool {
    text(p, key).is_some_and(|u| u.eq_ignore_ascii_case(uuid))
}

struct Adapter {
    path: String,
    powered: bool,
    discovering: bool,
    /// Advertisements active on it, any program's (not told by an old BlueZ).
    advertisements: Option<u8>,
}

/// The adapters that can do both halves: scan and connect (central), and
/// advertise and serve the payload (the two managers, which bluetoothd only
/// puts on an adapter whose controller can).
fn adapters(objects: &Objects) -> Vec<Adapter> {
    let mut out: Vec<Adapter> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let adapter = interfaces.get(ADAPTER)?;
            let advertising = interfaces.get(ADV_MANAGER)?;
            interfaces.get(GATT_MANAGER)?;
            // An older bluetoothd doesn't list roles; the managers then
            // stand for both.
            if adapter.contains_key("Roles") {
                let roles = list(adapter, "Roles");
                if !roles.contains(&"central") || !roles.contains(&"peripheral") {
                    return None;
                }
            }
            Some(Adapter {
                path: path.to_string(),
                powered: flag(adapter, "Powered"),
                discovering: flag(adapter, "Discovering"),
                advertisements: match advertising.get("ActiveInstances").map(|v| &**v) {
                    Some(Value::U8(n)) => Some(*n),
                    _ => None,
                },
            })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The adapter to use: the one in use while it is switched on, else one that
/// is, else whatever there is (to come back to once it is switched on; this
/// app never does that itself).
fn pick<'a>(adapters: &'a [Adapter], current: Option<&str>) -> Option<&'a Adapter> {
    let current = adapters.iter().find(|a| Some(a.path.as_str()) == current);
    current
        .filter(|a| a.powered)
        .or_else(|| adapters.iter().find(|a| a.powered))
        .or(current)
        .or(adapters.first())
}

/// The devices on `adapter` that advertise the service and are in range now
/// (`RSSI` is only there for what the running discovery found), minus what
/// the user blocked.
fn hosts(objects: &Objects, adapter: &str) -> Vec<String> {
    let mut out: Vec<String> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let dev = interfaces.get(DEVICE)?;
            let ours = text(dev, "Adapter") == Some(adapter)
                && dev.contains_key("RSSI")
                && !flag(dev, "Blocked")
                && list(dev, "UUIDs").iter().any(|u| u.eq_ignore_ascii_case(SERVICE));
            ours.then(|| path.to_string())
        })
        .collect();
    out.sort();
    out
}

/// The payload characteristic of the ReaMesh service among `dev`'s resolved
/// GATT objects.
fn payload_characteristic(objects: &Objects, dev: &str) -> Option<String> {
    let prefix = format!("{dev}/");
    objects.iter().find_map(|(path, interfaces)| {
        let c = interfaces.get(GATT_CHARACTERISTIC)?;
        if !path.as_str().starts_with(&prefix) || !has_uuid(c, "UUID", PAYLOAD) {
            return None;
        }
        let service = objects.iter().find(|(p, _)| Some(p.as_str()) == text(c, "Service"))?.1;
        has_uuid(service.get(GATT_SERVICE)?, "UUID", SERVICE).then(|| path.to_string())
    })
}

/// Connect to a host a discovery found and read its payload, all within
/// CONNECT_TIMEOUT, and leave the device as it was found. `ours`: the device
/// object appeared during our discovery (see `note_devices`).
fn read_payload(conn: &Connection, adapter: &str, dev: &str, ours: bool) -> Option<String> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    // What the device was to this machine before the read.
    let before = within(deadline, properties(conn, dev, DEVICE))?.ok()?;
    if flag(&before, "Blocked") {
        return None;
    }
    let connected = flag(&before, "Connected");
    let value = within(deadline, read_device(conn, dev, connected)).flatten();
    // Somebody else's connection (the user's, another program's) is left alone.
    if !connected {
        let bonded = |p: &Props| flag(p, "Paired") || flag(p, "Bonded");
        let stranger = ours && !bonded(&before) && !flag(&before, "Trusted");
        let _ = within(Instant::now() + SETUP_TIMEOUT, async {
            // Also calls off a connection that was still being made.
            let _ = call(conn, dev, DEVICE, "Disconnect", &()).await;
            // A host that answers the read with "insufficient encryption"
            // makes BlueZ raise the link's security on its own, which can
            // pair it. Unpairing over D-Bus means removing the device, and a
            // device that wasn't paired before loses nothing else by that.
            let paired_now = properties(conn, dev, DEVICE).await.is_ok_and(|p| bonded(&p));
            // Connecting also made bluetoothd keep the device for good. One
            // our own discovery brought in goes back to being a passer-by;
            // the discovery finds it again.
            if stranger || (paired_now && !bonded(&before)) {
                let _ = call(conn, adapter, ADAPTER, "RemoveDevice", &object(dev)).await;
            }
        });
    }
    value
}

async fn read_device(conn: &Connection, dev: &str, connected: bool) -> Option<String> {
    if !connected {
        call(conn, dev, DEVICE, "Connect", &()).await.ok()?;
    }
    // The host's GATT objects appear once bluetoothd has walked its services.
    loop {
        let p = properties(conn, dev, DEVICE).await.ok()?;
        if flag(&p, "ServicesResolved") {
            break;
        }
        if !flag(&p, "Connected") {
            return None;
        }
        async_io::Timer::after(RESOLVE_POLL).await;
    }
    let reply = call(conn, "/", OBJECT_MANAGER, "GetManagedObjects", &()).await.ok()?;
    let objects: Objects = reply.body().deserialize().ok()?;
    let characteristic = payload_characteristic(&objects, dev)?;
    // Always a read over the air; bluetoothd fetches a long value in full.
    let none = HashMap::<&str, Value>::new();
    let value: Vec<u8> =
        call(conn, &characteristic, GATT_CHARACTERISTIC, "ReadValue", &none).await.ok()?.body().deserialize().ok()?;
    if value.is_empty() || value.len() > MAX_PAYLOAD {
        return None;
    }
    String::from_utf8(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::Receiver;
    use zbus::message::Header;

    fn props(items: Vec<(&str, Value<'static>)>) -> Props {
        items.into_iter().map(|(k, v)| (k.to_string(), OwnedValue::try_from(v).unwrap())).collect()
    }

    fn objects(items: Vec<(&str, Vec<(&str, Props)>)>) -> Objects {
        items
            .into_iter()
            .map(|(path, interfaces)| {
                let interfaces = interfaces.into_iter().map(|(name, p)| (name.to_string(), p)).collect();
                (OwnedObjectPath::try_from(path).unwrap(), interfaces)
            })
            .collect()
    }

    fn path(p: &'static str) -> Value<'static> {
        Value::from(ObjectPath::from_static_str_unchecked(p))
    }

    #[test]
    fn long_reads_continue_from_their_offset() {
        assert_eq!(from_offset(b"abcdef", 0), Some(&b"abcdef"[..]));
        assert_eq!(from_offset(b"abcdef", 4), Some(&b"ef"[..]));
        assert_eq!(from_offset(b"abcdef", 6), Some(&b""[..]));
        assert_eq!(from_offset(b"abcdef", 7), None);
    }

    #[test]
    fn only_adapters_with_both_halves_count() {
        let on = || props(vec![("Powered", true.into()), ("Discovering", true.into())]);
        let managers = || vec![(ADV_MANAGER, props(vec![])), (GATT_MANAGER, props(vec![]))];
        let with = |adapter: Props, mut rest: Vec<(&'static str, Props)>| {
            rest.push((ADAPTER, adapter));
            rest
        };
        let o = objects(vec![
            // No advertising, no GATT server: a controller without LE peripheral.
            ("/org/bluez/hci0", with(on(), vec![])),
            ("/org/bluez/hci2", with(on(), managers())),
            (
                "/org/bluez/hci1",
                with(
                    props(vec![("Powered", false.into()), ("Roles", vec!["central", "peripheral"].into())]),
                    vec![(ADV_MANAGER, props(vec![("ActiveInstances", 2u8.into())])), (GATT_MANAGER, props(vec![]))],
                ),
            ),
            (
                "/org/bluez/hci3",
                with(props(vec![("Powered", true.into()), ("Roles", vec!["central"].into())]), managers()),
            ),
            ("/org/bluez/hci2/dev_00", vec![(DEVICE, props(vec![]))]),
        ]);
        let a = adapters(&o);
        assert_eq!(a.iter().map(|a| a.path.as_str()).collect::<Vec<_>>(), ["/org/bluez/hci1", "/org/bluez/hci2"]);
        assert!(!a[0].powered && !a[0].discovering && a[0].advertisements == Some(2));
        assert!(a[1].powered && a[1].discovering && a[1].advertisements.is_none());

        // Switched on beats first; the one in use is kept while it is on.
        let chosen = |a: &[Adapter], current| pick(a, current).map(|a| a.path.clone());
        assert_eq!(chosen(&a, None).as_deref(), Some("/org/bluez/hci2"));
        assert_eq!(chosen(&a, Some("/org/bluez/hci1")).as_deref(), Some("/org/bluez/hci2"));
        assert_eq!(chosen(&a[..1], None).as_deref(), Some("/org/bluez/hci1"));
        assert_eq!(chosen(&[], Some("/org/bluez/hci1")), None);
        let both_on = adapters(&objects(vec![
            ("/org/bluez/hci0", with(on(), managers())),
            ("/org/bluez/hci1", with(on(), managers())),
        ]));
        assert_eq!(chosen(&both_on, Some("/org/bluez/hci1")).as_deref(), Some("/org/bluez/hci1"));
    }

    #[test]
    fn only_hosts_in_range_are_read() {
        let dev = |adapter: &'static str, uuid: &'static str, extra: Vec<(&'static str, Value<'static>)>| {
            let battery = "0000180f-0000-1000-8000-00805f9b34fb";
            let mut p = vec![("Adapter", path(adapter)), ("UUIDs", vec![battery, uuid].into())];
            p.extend(extra);
            vec![(DEVICE, props(p))]
        };
        let rssi = || ("RSSI", Value::from(-60i16));
        let upper = "FC403C43-CEB1-43BD-9890-FD4198392FDD";
        let o = objects(vec![
            ("/org/bluez/hci0/dev_01", dev("/org/bluez/hci0", SERVICE, vec![rssi()])),
            ("/org/bluez/hci0/dev_02", dev("/org/bluez/hci0", upper, vec![rssi()])),
            // Remembered from earlier, not seen by this discovery.
            ("/org/bluez/hci0/dev_03", dev("/org/bluez/hci0", SERVICE, vec![])),
            ("/org/bluez/hci0/dev_04", dev("/org/bluez/hci0", SERVICE, vec![rssi(), ("Blocked", true.into())])),
            ("/org/bluez/hci0/dev_05", dev("/org/bluez/hci0", PAYLOAD, vec![rssi()])),
            ("/org/bluez/hci1/dev_06", dev("/org/bluez/hci1", SERVICE, vec![rssi()])),
        ]);
        assert_eq!(hosts(&o, "/org/bluez/hci0"), ["/org/bluez/hci0/dev_01", "/org/bluez/hci0/dev_02"]);
    }

    #[test]
    fn the_payload_is_found_under_its_own_service() {
        let service = |uuid: &'static str| vec![(GATT_SERVICE, props(vec![("UUID", uuid.into())]))];
        let chr = |uuid: &'static str, service: &'static str| {
            vec![(GATT_CHARACTERISTIC, props(vec![("UUID", uuid.into()), ("Service", path(service))]))]
        };
        let o = objects(vec![
            ("/org/bluez/hci0/dev_01/service0001", service("00001801-0000-1000-8000-00805f9b34fb")),
            // The right characteristic UUID under a foreign service.
            ("/org/bluez/hci0/dev_01/service0001/char0002", chr(PAYLOAD, "/org/bluez/hci0/dev_01/service0001")),
            ("/org/bluez/hci0/dev_01/service0010", service(SERVICE)),
            ("/org/bluez/hci0/dev_01/service0010/char0011", chr(SERVICE, "/org/bluez/hci0/dev_01/service0010")),
            ("/org/bluez/hci0/dev_01/service0010/char0013", chr(PAYLOAD, "/org/bluez/hci0/dev_01/service0010")),
            // Another device whose path only starts the same.
            ("/org/bluez/hci0/dev_011/service0010", service(SERVICE)),
            ("/org/bluez/hci0/dev_011/service0010/char0011", chr(PAYLOAD, "/org/bluez/hci0/dev_011/service0010")),
        ]);
        assert_eq!(
            payload_characteristic(&o, "/org/bluez/hci0/dev_01").as_deref(),
            Some("/org/bluez/hci0/dev_01/service0010/char0013")
        );
        assert_eq!(payload_characteristic(&o, "/org/bluez/hci0/dev_02"), None);
    }

    // ── A stand-in bluetoothd on a private bus ─────────────────────────────
    //
    // No radio is needed to check the D-Bus side: the worker registers with,
    // scans through and reads from a mock `org.bluez` that records what it
    // was asked and, like bluetoothd, reads the registered objects back.

    const HCI: &str = "/org/bluez/hci0";

    struct Bus {
        daemon: Child,
        address: String,
    }

    impl Bus {
        /// `None` where there is no dbus-daemon to run.
        fn start() -> Option<Bus> {
            let mut daemon = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut address = String::new();
            let read = daemon.stdout.take().map(|out| BufReader::new(out).read_line(&mut address));
            let bus = Bus { daemon, address: address.trim().to_string() };
            (matches!(read, Some(Ok(_))) && !bus.address.is_empty()).then_some(bus)
        }
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    #[derive(Default)]
    struct Mock {
        discovering: bool,
        filter_uuids: Vec<String>,
        filter_transport: String,
        /// The caller and what its `GetManagedObjects` held at registration.
        app: Option<(String, Objects)>,
        adv: Option<Props>,
        connected: HashSet<String>,
        paired: HashSet<String>,
        calls: Vec<String>,
    }

    type Shared = Arc<Mutex<Mock>>;

    #[derive(Debug, zbus::DBusError)]
    #[zbus(prefix = "org.bluez.Error")]
    enum BluezError {
        #[zbus(error)]
        ZBus(zbus::Error),
        InProgress(String),
        Failed(String),
    }

    struct MockAdapter(Shared);

    #[interface(name = "org.bluez.Adapter1")]
    impl MockAdapter {
        #[zbus(property)]
        fn powered(&self) -> bool {
            true
        }

        #[zbus(property)]
        fn discovering(&self) -> bool {
            self.0.lock().unwrap().discovering
        }

        #[zbus(property)]
        fn roles(&self) -> Vec<String> {
            vec!["central".into(), "peripheral".into()]
        }

        fn set_discovery_filter(&self, filter: Props) {
            let mut m = self.0.lock().unwrap();
            m.filter_uuids = list(&filter, "UUIDs").into_iter().map(str::to_string).collect();
            m.filter_transport = text(&filter, "Transport").unwrap_or_default().to_string();
        }

        fn start_discovery(&self) -> Result<(), BluezError> {
            if std::mem::replace(&mut self.0.lock().unwrap().discovering, true) {
                return Err(BluezError::InProgress("Operation already in progress".into()));
            }
            Ok(())
        }

        fn stop_discovery(&self) -> Result<(), BluezError> {
            if !std::mem::replace(&mut self.0.lock().unwrap().discovering, false) {
                return Err(BluezError::Failed("No discovery started".into()));
            }
            Ok(())
        }

        fn remove_device(&self, device: OwnedObjectPath) {
            self.0.lock().unwrap().calls.push(format!("remove {device}"));
        }
    }

    struct MockAdvertising(Shared);

    #[interface(name = "org.bluez.LEAdvertisingManager1")]
    impl MockAdvertising {
        #[zbus(property)]
        fn active_instances(&self) -> u8 {
            self.0.lock().unwrap().adv.is_some() as u8
        }

        async fn register_advertisement(
            &self,
            advertisement: OwnedObjectPath,
            _options: Props,
            #[zbus(header)] header: Header<'_>,
            #[zbus(connection)] conn: &Connection,
        ) -> Result<(), BluezError> {
            let sender = header.sender().ok_or_else(|| BluezError::Failed("no sender".into()))?.to_string();
            let interface = "org.bluez.LEAdvertisement1";
            let p: Props = conn
                .call_method(Some(sender.as_str()), &advertisement, Some(PROPERTIES), "GetAll", &interface)
                .await?
                .body()
                .deserialize()?;
            self.0.lock().unwrap().adv = Some(p);
            Ok(())
        }

        fn unregister_advertisement(&self, _advertisement: OwnedObjectPath) -> Result<(), BluezError> {
            let gone = self.0.lock().unwrap().adv.take();
            gone.map(drop).ok_or_else(|| BluezError::Failed("Does Not Exist".into()))
        }
    }

    struct MockGatt(Shared);

    #[interface(name = "org.bluez.GattManager1")]
    impl MockGatt {
        async fn register_application(
            &self,
            application: OwnedObjectPath,
            _options: Props,
            #[zbus(header)] header: Header<'_>,
            #[zbus(connection)] conn: &Connection,
        ) -> Result<(), BluezError> {
            let sender = header.sender().ok_or_else(|| BluezError::Failed("no sender".into()))?.to_string();
            let o: Objects = conn
                .call_method(Some(sender.as_str()), &application, Some(OBJECT_MANAGER), "GetManagedObjects", &())
                .await?
                .body()
                .deserialize()?;
            self.0.lock().unwrap().app = Some((sender, o));
            Ok(())
        }

        fn unregister_application(&self, _application: OwnedObjectPath) -> Result<(), BluezError> {
            let gone = self.0.lock().unwrap().app.take();
            gone.map(drop).ok_or_else(|| BluezError::Failed("Does Not Exist".into()))
        }
    }

    struct MockDevice {
        mock: Shared,
        path: String,
        uuid: &'static str,
        in_range: bool,
        /// A host that demands encryption: BlueZ pairs it while reading.
        pairs_on_read: bool,
    }

    #[interface(name = "org.bluez.Device1")]
    impl MockDevice {
        #[zbus(property)]
        fn adapter(&self) -> OwnedObjectPath {
            object(HCI).into()
        }

        #[zbus(property, name = "UUIDs")]
        fn uuids(&self) -> Vec<String> {
            vec![self.uuid.to_string()]
        }

        // Only there for a device the running discovery found.
        #[zbus(property, name = "RSSI")]
        fn rssi(&self) -> zbus::fdo::Result<i16> {
            self.in_range.then_some(-50).ok_or_else(|| zbus::fdo::Error::Failed("not in range".into()))
        }

        #[zbus(property)]
        fn paired(&self) -> bool {
            self.mock.lock().unwrap().paired.contains(&self.path)
        }

        #[zbus(property)]
        fn connected(&self) -> bool {
            self.mock.lock().unwrap().connected.contains(&self.path)
        }

        #[zbus(property)]
        fn services_resolved(&self) -> bool {
            self.connected()
        }

        fn connect(&self) {
            let mut m = self.mock.lock().unwrap();
            m.connected.insert(self.path.clone());
            if self.pairs_on_read {
                m.paired.insert(self.path.clone());
            }
            m.calls.push(format!("connect {}", self.path));
        }

        fn disconnect(&self) {
            let mut m = self.mock.lock().unwrap();
            m.connected.remove(&self.path);
            m.calls.push(format!("disconnect {}", self.path));
        }
    }

    struct MockService;

    #[interface(name = "org.bluez.GattService1")]
    impl MockService {
        #[zbus(property, name = "UUID")]
        fn uuid(&self) -> String {
            SERVICE.into()
        }
    }

    struct MockCharacteristic {
        service: String,
        value: String,
    }

    #[interface(name = "org.bluez.GattCharacteristic1")]
    impl MockCharacteristic {
        #[zbus(property, name = "UUID")]
        fn uuid(&self) -> String {
            PAYLOAD.into()
        }

        #[zbus(property)]
        fn service(&self) -> OwnedObjectPath {
            object(&self.service).into()
        }

        fn read_value(&self, _options: Props) -> Vec<u8> {
            self.value.clone().into_bytes()
        }
    }

    fn soon() -> Instant {
        Instant::now() + SETUP_TIMEOUT
    }

    /// A host as the mock knows it.
    #[derive(Clone, Copy, Default)]
    struct Host {
        /// Also the payload it serves.
        name: &'static str,
        /// The UUID it advertises.
        uuid: &'static str,
        in_range: bool,
        paired: bool,
        connected: bool,
        pairs_on_read: bool,
    }

    fn host(name: &'static str) -> Host {
        Host { name, uuid: SERVICE, in_range: true, ..Host::default() }
    }

    /// Put a device object (with its resolved GATT objects) on the mock.
    fn add_host(conn: &Connection, mock: &Shared, h: Host) {
        within(soon(), async {
            let server = conn.object_server();
            let path = format!("{HCI}/dev_{}", h.name);
            {
                let mut m = mock.lock().unwrap();
                if h.connected {
                    m.connected.insert(path.clone());
                }
                if h.paired {
                    m.paired.insert(path.clone());
                }
            }
            let service = format!("{path}/service0010");
            server.at(service.as_str(), MockService).await?;
            let value = h.name.to_string();
            server.at(format!("{service}/char0011"), MockCharacteristic { service, value }).await?;
            let (uuid, in_range, pairs_on_read) = (h.uuid, h.in_range, h.pairs_on_read);
            let device = MockDevice { mock: mock.clone(), path: path.clone(), uuid, in_range, pairs_on_read };
            server.at(path, device).await?;
            zbus::Result::Ok(())
        })
        .expect("adding a mock host timed out")
        .expect("adding a mock host");
    }

    /// A bluetoothd with one adapter and the given hosts around it.
    fn bluetoothd(bus: &Bus, hosts: &[Host]) -> (Connection, Shared) {
        let mock = Shared::default();
        let conn = within(soon(), async {
            let conn = zbus::connection::Builder::address(bus.address.as_str())?.build().await?;
            let server = conn.object_server();
            server.at("/", zbus::fdo::ObjectManager).await?;
            server.at(HCI, MockAdapter(mock.clone())).await?;
            server.at(HCI, MockAdvertising(mock.clone())).await?;
            server.at(HCI, MockGatt(mock.clone())).await?;
            zbus::Result::Ok(conn)
        })
        .expect("mock bluetoothd timed out")
        .expect("mock bluetoothd");
        for &h in hosts {
            add_host(&conn, &mock, h);
        }
        // Last, so that everything is there when the name appears.
        within(soon(), conn.request_name(BLUEZ)).expect("name request timed out").expect("name request");
        (conn, mock)
    }

    /// Hand the worker the next `n` messages its threads post, keeping what
    /// the reads among them returned.
    fn pump(w: &mut Worker, rx: &Receiver<Msg>, n: usize) -> Vec<(String, Option<String>)> {
        let mut reads = Vec::new();
        for _ in 0..n {
            let msg = rx.recv_timeout(Duration::from_secs(20)).expect("the worker's threads went quiet");
            if let Msg::Read(dev, value) = &msg {
                reads.push((dev.clone(), value.clone()));
            }
            w.handle(msg);
        }
        reads.sort();
        reads
    }

    #[test]
    fn worker_against_a_mock_bluetoothd() {
        let Some(bus) = Bus::start() else {
            eprintln!("skipped: no dbus-daemon to run a private bus with");
            return;
        };
        let (tx, rx) = channel();
        let mut w = Worker::new(tx, Some(bus.address.clone()));

        // A bus without BlueZ: unsupported.
        w.reconcile();
        assert!(!supported() && authorization() == -1);

        let (bluez, mock) = bluetoothd(
            &bus,
            &[
                // Known to BlueZ before any discovery of ours.
                host("cached"),
                Host { pairs_on_read: true, ..host("cached_strict") },
                Host { paired: true, ..host("paired") },
                Host { paired: true, connected: true, ..host("in_use") },
                Host { uuid: PAYLOAD, ..host("other") },
                Host { in_range: false, ..host("gone") },
            ],
        );
        // Not looked for again before the next probe is due.
        w.reconcile();
        assert!(!supported());
        w.probed = None;
        w.reconcile();
        assert!(supported());
        assert_eq!(w.adapter.as_deref(), Some(HCI));
        assert_eq!(authorization(), 0);

        // ── Hosting ────────────────────────────────────────────────────────
        let payload = br#"{"v":3,"ips":["192.168.1.20"],"tcp":4001,"quic":4002}"#;
        w.handle(Msg::Advertise(Some(payload.to_vec())));
        w.reconcile();
        assert_eq!(authorization(), 3);
        let me = w.conn.as_ref().unwrap().unique_name().unwrap().to_string();
        {
            let m = mock.lock().unwrap();
            let (sender, app) = m.app.as_ref().expect("no GATT application registered");
            assert_eq!(sender, &me);
            assert_eq!(app.len(), 2, "{:?}", app.keys());
            let service = &app[&OwnedObjectPath::try_from(SERVICE_PATH).unwrap()][GATT_SERVICE];
            assert!(has_uuid(service, "UUID", SERVICE) && flag(service, "Primary"));
            let chr = &app[&OwnedObjectPath::try_from(CHARACTERISTIC_PATH).unwrap()][GATT_CHARACTERISTIC];
            assert!(has_uuid(chr, "UUID", PAYLOAD));
            assert_eq!(text(chr, "Service"), Some(SERVICE_PATH));
            assert_eq!(list(chr, "Flags"), ["read"]);

            let adv = m.adv.as_ref().expect("no advertisement registered");
            assert_eq!(text(adv, "Type"), Some("peripheral"));
            assert_eq!(list(adv, "ServiceUUIDs"), [SERVICE]);
            assert!(flag(adv, "Discoverable"));
            // The service UUID and nothing that names the machine.
            let mut keys: Vec<_> = adv.keys().map(String::as_str).collect();
            keys.sort();
            assert_eq!(keys, ["Discoverable", "ServiceUUIDs", "Type"]);
        }
        // A guest's read, as bluetoothd passes it on.
        let read = |offset: usize| {
            within(soon(), async {
                let guest = path("/org/bluez/hci0/dev_guest");
                let mut options = HashMap::from([("device", guest), ("mtu", 517u16.into())]);
                if offset > 0 {
                    options.insert("offset", (offset as u16).into());
                }
                let (to, interface) = (Some(me.as_str()), Some(GATT_CHARACTERISTIC));
                let reply = bluez.call_method(to, CHARACTERISTIC_PATH, interface, "ReadValue", &options).await?;
                reply.body().deserialize::<Vec<u8>>()
            })
            .expect("read timed out")
        };
        assert_eq!(read(0).unwrap(), payload);
        assert_eq!(read(20).unwrap(), &payload[20..]);
        assert_eq!(read(payload.len()).unwrap(), b"");
        assert!(is_error(&read(payload.len() + 1).unwrap_err(), "org.bluez.Error.InvalidOffset"));
        // Re-hosting swaps the payload under the registered service.
        w.handle(Msg::Advertise(Some(b"again".to_vec())));
        w.reconcile();
        assert_eq!(read(0).unwrap(), b"again");

        // bluetoothd drops the advertisement on its own: both are made anew.
        mock.lock().unwrap().adv = None;
        let (to, interface) = (Some(me.as_str()), Some("org.bluez.LEAdvertisement1"));
        within(soon(), bluez.call_method(to, ADV_PATH, interface, "Release", &())).expect("release timed out").unwrap();
        pump(&mut w, &rx, 1);
        assert!(w.advertised.is_none() && mock.lock().unwrap().app.is_none());
        w.reconcile();
        assert!(mock.lock().unwrap().app.is_some() && mock.lock().unwrap().adv.is_some());
        // So is one that vanished without a word (a quick power cycle).
        mock.lock().unwrap().adv = None;
        w.reconcile();
        assert!(mock.lock().unwrap().adv.is_some());

        w.handle(Msg::Advertise(None));
        w.reconcile();
        {
            let m = mock.lock().unwrap();
            assert!(m.app.is_none() && m.adv.is_none() && w.advertised.is_none());
        }
        assert!(read(0).is_err(), "the payload is still served after hosting ended");

        // ── Scanning ───────────────────────────────────────────────────────
        w.handle(Msg::Scan(true));
        w.reconcile();
        {
            let m = mock.lock().unwrap();
            assert!(m.discovering);
            assert_eq!(m.filter_uuids, [SERVICE]);
            assert_eq!(m.filter_transport, "le");
        }
        let dev = |name: &str| format!("{HCI}/dev_{name}");
        let read_from = |names: &[&str]| -> Vec<(String, Option<String>)> {
            names.iter().map(|n| (dev(n), Some(n.to_string()))).collect()
        };
        let did = |what: &str, names: &[&str]| names.iter().map(|n| format!("{what} {}", dev(n))).collect::<Vec<_>>();
        let calls = || {
            let mut m = mock.lock().unwrap();
            let mut c = std::mem::take(&mut m.calls);
            c.sort();
            c
        };
        // Three at a time, in path order: `paired` waits for the next tick.
        assert_eq!(pump(&mut w, &rx, 3), read_from(&["cached", "cached_strict", "in_use"]));
        // A device BlueZ knew before is only disconnected, unless the read
        // got it paired; one that was in use is not touched at all.
        let both = ["cached", "cached_strict"];
        let want = [did("connect", &both), did("disconnect", &both), did("remove", &["cached_strict"])];
        assert_eq!(calls(), want.concat());
        assert_eq!(mock.lock().unwrap().connected.iter().collect::<Vec<_>>(), [&dev("in_use")]);

        // Hosts our discovery brings in are forgotten again after their read,
        // a paired one only disconnected; nothing advertising another service
        // or out of range is connected to.
        add_host(&bluez, &mock, host("stranger"));
        add_host(&bluez, &mock, Host { pairs_on_read: true, ..host("strict") });
        w.reconcile();
        assert_eq!(pump(&mut w, &rx, 3), read_from(&["paired", "stranger", "strict"]));
        let all = ["paired", "stranger", "strict"];
        let want = [did("connect", &all), did("disconnect", &all), did("remove", &["stranger", "strict"])];
        assert_eq!(calls(), want.concat());
        assert_eq!(mock.lock().unwrap().connected.iter().collect::<Vec<_>>(), [&dev("in_use")]);
        // Still there on the next tick, but read a moment ago.
        w.reconcile();
        assert!(w.connecting.is_empty() && mock.lock().unwrap().calls.is_empty());

        w.handle(Msg::Scan(false));
        w.reconcile();
        assert!(!mock.lock().unwrap().discovering && w.scanning.is_none());

        // ── bluetoothd restarts while hosting ──────────────────────────────
        w.handle(Msg::Advertise(Some(payload.to_vec())));
        w.reconcile();
        assert!(w.advertised.is_some());
        within(soon(), bluez.close()).expect("close timed out").unwrap();
        w.reconcile();
        assert!(!supported() && w.advertised.is_none() && w.bluez.is_none());
        let (_bluez, mock) = bluetoothd(&bus, &[]);
        w.probed = None;
        w.reconcile();
        assert!(supported() && mock.lock().unwrap().adv.is_some() && mock.lock().unwrap().app.is_some());

        // ── The bus itself goes away ───────────────────────────────────────
        drop(bus);
        w.reconcile();
        assert!(!supported() && w.conn.is_none() && w.advertised.is_none());
    }
}
