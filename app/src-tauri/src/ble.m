#import "ble.h"
#import <CoreBluetooth/CoreBluetooth.h>

// See ble.h. Everything runs on one serial queue: CoreBluetooth calls its
// delegates there, and the C entry points hop onto it, so no state is ever
// touched from two threads.

static NSString *const kServiceUUID = @"FC403C43-CEB1-43BD-9890-FD4198392FDD";
static NSString *const kPayloadUUID = @"176F36B1-74E7-4B2F-A3C4-24620E09B8CD";
static const NSUInteger kMaxPayload = 512;
// The same host is read again at most this often (its ports change when it
// re-hosts), and a read that hangs is given up after kConnectTimeout.
static const NSTimeInterval kRereadInterval = 20;
static const NSTimeInterval kConnectTimeout = 8;
// Advertisements are readable by anyone nearby: bound the work one can cause.
static const NSUInteger kMaxConnecting = 3;
static const NSUInteger kMaxRemembered = 256;

@interface ReaMeshBle
    : NSObject <CBPeripheralManagerDelegate, CBCentralManagerDelegate, CBPeripheralDelegate>
@property (strong) dispatch_queue_t queue;
@property (assign) ReaMeshBleFound found;
// Host side.
@property (strong) CBPeripheralManager *pm;
@property (strong) CBMutableService *service;
@property (assign) BOOL serviceAdded;
@property (strong) NSData *payload; // nil = not advertising
// Scanner side.
@property (strong) CBCentralManager *cm;
@property (assign) BOOL wantScan;
@property (strong) NSMutableDictionary<NSUUID *, NSDate *> *lastRead;
@property (strong) NSMutableDictionary<NSUUID *, CBPeripheral *> *connecting;
@end

static ReaMeshBle *gBle = nil;

@implementation ReaMeshBle

- (instancetype)init {
    self = [super init];
    if (!self) return self;
    _queue = dispatch_queue_create("com.reamesh.ble", DISPATCH_QUEUE_SERIAL);
    _lastRead = [NSMutableDictionary dictionary];
    _connecting = [NSMutableDictionary dictionary];
    return self;
}

#pragma mark - Host: advertise

- (void)setAdvertisedPayload:(NSData *)data {
    self.payload = data;
    if (!data) {
        if (self.pm.isAdvertising) [self.pm stopAdvertising];
        [self.pm removeAllServices];
        self.service = nil;
        self.serviceAdded = NO;
        return;
    }
    if (!self.pm) {
        // First use: this is where macOS asks for the Bluetooth permission.
        self.pm = [[CBPeripheralManager alloc] initWithDelegate:self queue:self.queue options:nil];
        return; // continues in peripheralManagerDidUpdateState:
    }
    [self startAdvertisingIfReady];
}

- (void)startAdvertisingIfReady {
    if (!self.payload || self.pm.state != CBManagerStatePoweredOn) return;
    if (!self.service) {
        // No static value: reads are answered from `payload`, so a re-host's
        // new ports are served without rebuilding the service.
        CBMutableCharacteristic *c = [[CBMutableCharacteristic alloc]
            initWithType:[CBUUID UUIDWithString:kPayloadUUID]
              properties:CBCharacteristicPropertyRead
                   value:nil
             permissions:CBAttributePermissionsReadable];
        CBMutableService *s =
            [[CBMutableService alloc] initWithType:[CBUUID UUIDWithString:kServiceUUID] primary:YES];
        s.characteristics = @[ c ];
        self.service = s;
        self.serviceAdded = NO;
        [self.pm addService:s];
        return; // continues in didAddService:
    }
    if (self.serviceAdded && !self.pm.isAdvertising) {
        // Only the service UUID: no local name, so the advertisement doesn't
        // carry the computer's name either.
        [self.pm startAdvertising:@{
            CBAdvertisementDataServiceUUIDsKey : @[ [CBUUID UUIDWithString:kServiceUUID] ]
        }];
    }
}

- (void)peripheralManagerDidUpdateState:(CBPeripheralManager *)peripheral {
    if (peripheral.state == CBManagerStatePoweredOn) {
        [self startAdvertisingIfReady];
    } else {
        // Bluetooth off or the permission withdrawn: the stack dropped the
        // service; publish it again when it comes back.
        self.service = nil;
        self.serviceAdded = NO;
    }
}

- (void)peripheralManager:(CBPeripheralManager *)peripheral
            didAddService:(CBService *)service
                    error:(NSError *)error {
    if (error || service != self.service) return;
    self.serviceAdded = YES;
    [self startAdvertisingIfReady];
}

- (void)peripheralManager:(CBPeripheralManager *)peripheral
    didReceiveReadRequest:(CBATTRequest *)request {
    NSData *data = self.payload;
    if (!data || ![request.characteristic.UUID isEqual:[CBUUID UUIDWithString:kPayloadUUID]]) {
        [peripheral respondToRequest:request withResult:CBATTErrorAttributeNotFound];
        return;
    }
    if (request.offset > data.length) {
        [peripheral respondToRequest:request withResult:CBATTErrorInvalidOffset];
        return;
    }
    request.value = [data subdataWithRange:NSMakeRange(request.offset, data.length - request.offset)];
    [peripheral respondToRequest:request withResult:CBATTErrorSuccess];
}

#pragma mark - Scanner: find and read

- (void)setScanning:(BOOL)on {
    self.wantScan = on;
    if (on && !self.cm) {
        // First use: this is where macOS asks for the Bluetooth permission.
        self.cm = [[CBCentralManager alloc] initWithDelegate:self queue:self.queue options:nil];
        return; // continues in centralManagerDidUpdateState:
    }
    [self updateScan];
}

- (void)updateScan {
    if (!self.cm || self.cm.state != CBManagerStatePoweredOn) return;
    if (self.wantScan && !self.cm.isScanning) {
        // Duplicates on, so a host that re-hosts (new ports) is read again
        // after kRereadInterval; the rate limit is ours, below.
        [self.cm scanForPeripheralsWithServices:@[ [CBUUID UUIDWithString:kServiceUUID] ]
                                        options:@{CBCentralManagerScanOptionAllowDuplicatesKey : @YES}];
    } else if (!self.wantScan) {
        if (self.cm.isScanning) [self.cm stopScan];
        for (CBPeripheral *p in self.connecting.allValues) [self.cm cancelPeripheralConnection:p];
        [self.connecting removeAllObjects];
    }
}

- (void)centralManagerDidUpdateState:(CBCentralManager *)central {
    if (central.state != CBManagerStatePoweredOn) [self.connecting removeAllObjects];
    [self updateScan];
}

- (void)centralManager:(CBCentralManager *)central
    didDiscoverPeripheral:(CBPeripheral *)peripheral
        advertisementData:(NSDictionary<NSString *, id> *)advertisementData
                     RSSI:(NSNumber *)RSSI {
    if (!self.wantScan) return;
    NSUUID *pid = peripheral.identifier;
    if (self.connecting[pid] || self.connecting.count >= kMaxConnecting) return;
    NSDate *last = self.lastRead[pid];
    if (last && -[last timeIntervalSinceNow] < kRereadInterval) return;
    if (self.lastRead.count >= kMaxRemembered) [self forgetOldReads];
    // Stamped before the attempt, so a host that fails to answer is retried
    // on the same schedule instead of on every advertisement.
    self.lastRead[pid] = [NSDate date];
    self.connecting[pid] = peripheral;
    peripheral.delegate = self;
    [central connectPeripheral:peripheral options:nil];
    __weak ReaMeshBle *weakSelf = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(kConnectTimeout * NSEC_PER_SEC)),
                   self.queue, ^{
        ReaMeshBle *s = weakSelf;
        if (s && s.connecting[pid] == peripheral) [s finish:peripheral];
    });
}

- (void)forgetOldReads {
    NSMutableArray *old = [NSMutableArray array];
    [self.lastRead enumerateKeysAndObjectsUsingBlock:^(NSUUID *k, NSDate *at, BOOL *stop) {
        (void)stop;
        if (-[at timeIntervalSinceNow] >= kRereadInterval) [old addObject:k];
    }];
    [self.lastRead removeObjectsForKeys:old];
    if (self.lastRead.count >= kMaxRemembered) [self.lastRead removeAllObjects];
}

- (void)finish:(CBPeripheral *)peripheral {
    [self.connecting removeObjectForKey:peripheral.identifier];
    [self.cm cancelPeripheralConnection:peripheral];
}

- (void)centralManager:(CBCentralManager *)central didConnectPeripheral:(CBPeripheral *)peripheral {
    [peripheral discoverServices:@[ [CBUUID UUIDWithString:kServiceUUID] ]];
}

- (void)centralManager:(CBCentralManager *)central
    didFailToConnectPeripheral:(CBPeripheral *)peripheral
                         error:(NSError *)error {
    [self.connecting removeObjectForKey:peripheral.identifier];
}

- (void)centralManager:(CBCentralManager *)central
    didDisconnectPeripheral:(CBPeripheral *)peripheral
                      error:(NSError *)error {
    [self.connecting removeObjectForKey:peripheral.identifier];
}

- (void)peripheral:(CBPeripheral *)peripheral didDiscoverServices:(NSError *)error {
    for (CBService *s in peripheral.services) {
        if ([s.UUID isEqual:[CBUUID UUIDWithString:kServiceUUID]]) {
            [peripheral discoverCharacteristics:@[ [CBUUID UUIDWithString:kPayloadUUID] ] forService:s];
            return;
        }
    }
    [self finish:peripheral];
}

- (void)peripheral:(CBPeripheral *)peripheral
    didDiscoverCharacteristicsForService:(CBService *)service
                                   error:(NSError *)error {
    for (CBCharacteristic *c in service.characteristics) {
        if ([c.UUID isEqual:[CBUUID UUIDWithString:kPayloadUUID]]) {
            [peripheral readValueForCharacteristic:c];
            return;
        }
    }
    [self finish:peripheral];
}

- (void)peripheral:(CBPeripheral *)peripheral
    didUpdateValueForCharacteristic:(CBCharacteristic *)characteristic
                              error:(NSError *)error {
    NSData *v = characteristic.value;
    if (!error && v.length > 0 && v.length <= kMaxPayload && self.found && self.wantScan) {
        NSString *s = [[NSString alloc] initWithData:v encoding:NSUTF8StringEncoding];
        if (s) self.found(s.UTF8String);
    }
    [self finish:peripheral];
}

@end

static ReaMeshBle *ble(void) {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        gBle = [[ReaMeshBle alloc] init];
    });
    return gBle;
}

void reamesh_ble_init(ReaMeshBleFound found) {
    ReaMeshBle *b = ble();
    dispatch_async(b.queue, ^{
        b.found = found;
    });
}

int32_t reamesh_ble_authorization(void) {
    switch (CBManager.authorization) {
        case CBManagerAuthorizationNotDetermined:
            return kReaMeshBleNotDetermined;
        case CBManagerAuthorizationRestricted:
            return kReaMeshBleRestricted;
        case CBManagerAuthorizationDenied:
            return kReaMeshBleDenied;
        case CBManagerAuthorizationAllowedAlways:
            return kReaMeshBleAllowed;
    }
    return kReaMeshBleUnsupported;
}

void reamesh_ble_advertise(const char *payload) {
    NSData *data = nil;
    if (payload && payload[0]) {
        data = [NSData dataWithBytes:payload length:strlen(payload)];
        if (data.length > kMaxPayload) data = nil;
    }
    ReaMeshBle *b = ble();
    dispatch_async(b.queue, ^{
        // Same payload again (the webview re-sends on every state change):
        // nothing to do.
        if ((data == nil && b.payload == nil) || [data isEqualToData:b.payload]) return;
        [b setAdvertisedPayload:data];
    });
}

void reamesh_ble_scan(int32_t on) {
    ReaMeshBle *b = ble();
    dispatch_async(b.queue, ^{
        if ((on != 0) == b.wantScan && (on == 0 || b.cm)) return;
        [b setScanning:on != 0];
    });
}
