//! Bluetooth device battery reader: enumerates paired devices and reads the
//! GATT "Battery Service" (0x180F / Battery Level 0x2A19) via the Windows
//! Runtime. Classic headsets (A2DP/HFP) get their percentage from the
//! Hands-Free PnP property Windows already fills.
//!
//! Performance design:
//!   - every WinRT async op is awaited with a hard timeout (a hung BT stack
//!     can no longer freeze the poll thread forever)
//!   - service/characteristic discovery uses the system cache; only the
//!     battery *value* is read uncached (3 radio round-trips -> 1)
//!   - connected devices are scanned in parallel worker threads
//!   - devices whose battery characteristic supports Notify are subscribed
//!     once; pushed values land in NOTIFY_CACHE and repaint instantly
//!   - a DeviceWatcher fires refreshes on connect/disconnect so the strip
//!     reacts within ~1s of a device appearing instead of at the next poll

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristic, GattCharacteristicProperties, GattCharacteristicUuids,
    GattCommunicationStatus, GattServiceUuids, GattSession, GattValueChangedEventArgs,
};
use windows::Devices::Bluetooth::{
    BluetoothCacheMode, BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice,
};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationUpdate, DeviceWatcher};
use windows::Foundation::{IAsyncOperation, TypedEventHandler};
use windows::Storage::Streams::DataReader;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_ALLCLASSES, DIGCF_PRESENT, SETUP_DI_GET_CLASS_DEVS_FLAGS, SP_DEVINFO_DATA,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDevicePropertyW,
};
use windows::Win32::Devices::Properties::{DEVPROP_TYPE_BYTE, DEVPROPKEY, DEVPROPTYPE};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::{GUID, w};

use crate::model::Device;

/// Undocumented PnP key Windows uses for HFP headset battery (byte 0..=100).
const HFP_BATTERY_KEY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5),
    pid: 2,
};

/// Hard ceiling for one WinRT async operation. Normal ops finish in <100ms;
/// this only matters when the BT stack wedges, and it must stay well below
/// the poll interval so one bad device cannot stall a whole scan.
const OP_TIMEOUT: Duration = Duration::from_secs(5);
/// Parallel worker threads used for the per-device GATT reads.
const SCAN_WORKERS: usize = 3;
/// How long a Notify-pushed battery value stays trusted without re-read.
const NOTIFY_TTL: Duration = Duration::from_secs(30 * 60);

/// Battery percentages pushed by devices via GATT notifications.
static NOTIFY_CACHE: LazyLock<Mutex<HashMap<String, (u8, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Everything that must outlive a subscription for its events to keep firing.
struct NotifySub {
    _dev: BluetoothLEDevice,
    _session: Option<GattSession>,
    _token: windows::Foundation::EventRegistrationToken,
}
static NOTIFY_SUBS: LazyLock<Mutex<HashMap<String, NotifySub>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// DeviceWatchers that turn connect/disconnect into an instant refresh kick.
static WATCHERS: LazyLock<Mutex<Vec<DeviceWatcher>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static LAST_KICK: LazyLock<Mutex<Instant>> =
    LazyLock::new(|| Mutex::new(Instant::now() - Duration::from_secs(60)));

/// Await a WinRT async operation with a deadline. Returns true when the op
/// completed (then '.get()' returns immediately); false on error/cancel/
/// timeout (the op is cancelled so it stops consuming radio time).
fn op_wait<T: windows::core::RuntimeType>(op: &IAsyncOperation<T>, timeout: Duration) -> bool {
    use windows::Foundation::AsyncStatus;
    let start = Instant::now();
    loop {
        match op.Status() {
            Ok(AsyncStatus::Completed) => return true,
            Ok(AsyncStatus::Started) => {}
            Ok(_) => return false, // Canceled / Error
            Err(_) => return false,
        }
        if start.elapsed() >= timeout {
            let _ = op.Cancel();
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Run an async WinRT call and unwrap it, or None on any failure/timeout.
fn op_result<T: windows::core::RuntimeType>(
    op: windows::core::Result<IAsyncOperation<T>>,
) -> Option<T> {
    let op = op.ok()?;
    if op_wait(&op, OP_TIMEOUT) {
        op.get().ok()
    } else {
        None
    }
}

pub fn enumerate() -> Vec<Device> {
    let mut out: Vec<Device> = Vec::new();

    // ---- collect the paired-device lists (fast, cached by the system) ----
    // 走 op_result：这些 WinRT 异步调用同样必须有硬超时，否则蓝牙栈卡死时
    // 整个轮询线程会永久阻塞在 .get() 上（与模块头声明的设计相矛盾）。
    let mut le_infos = Vec::new();
    if let Ok(selector) = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)
        && let Some(collection) = op_result(DeviceInformation::FindAllAsyncAqsFilter(&selector))
    {
        let size = collection.Size().unwrap_or(0);
        for i in 0..size {
            if let Ok(info) = collection.GetAt(i) {
                le_infos.push(info);
            }
        }
    }
    let mut classic_infos = Vec::new();
    if let Ok(selector) = BluetoothDevice::GetDeviceSelectorFromPairingState(true)
        && let Some(collection) = op_result(DeviceInformation::FindAllAsyncAqsFilter(&selector))
    {
        let size = collection.Size().unwrap_or(0);
        for i in 0..size {
            if let Ok(info) = collection.GetAt(i) {
                classic_infos.push(info);
            }
        }
    }

    // ---- scan LE devices on parallel workers (GATT reads dominate) ----
    let chunks: Vec<Vec<DeviceInformation>> = split_for_workers(&le_infos, SCAN_WORKERS);
    let results: Vec<Device> = std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| {
                scope.spawn(move || {
                    // MTA so WinRT calls are legal from this worker thread.
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    }
                    let mut seen: HashSet<String> = HashSet::new();
                    let mut local = Vec::new();
                    for info in &chunk {
                        if let Some(d) = visit_le(info, &mut seen) {
                            local.push(d);
                        }
                    }
                    local
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    out.extend(results);

    // ---- classic devices: cheap (no GATT), do them inline ----
    let mut seen: HashSet<String> = out.iter().map(|d| d.address.clone()).collect();
    for info in &classic_infos {
        if let Some(d) = visit_classic(info, &mut seen) {
            out.push(d);
        }
    }

    // ---- overlay Notify-pushed values (fresher than a poll read) ----
    if let Ok(cache) = NOTIFY_CACHE.lock() {
        for d in &mut out {
            if d.battery.is_none()
                && d.connected
                && let Some((pct, at)) = cache.get(&d.address)
                && at.elapsed() < NOTIFY_TTL
            {
                d.battery = Some(*pct);
                d.status = "ok";
            }
        }
    }

    let hfp = hfp_battery_map();
    for d in &mut out {
        if !d.connected {
            // 断开的设备不保留电量信息（HFP 属性是系统缓存的历史值）。
            d.battery = None;
            if d.status == "ok" {
                d.status = "offline";
            }
            continue;
        }
        if d.battery.is_some() {
            continue;
        }
        if let Some(pct) = hfp.get(&normalize_bt_addr(&d.address)) {
            d.battery = Some(*pct);
            d.status = "ok";
        }
    }

    out.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

fn split_for_workers<T: Clone>(items: &[T], workers: usize) -> Vec<Vec<T>> {
    if items.is_empty() {
        return Vec::new();
    }
    let n = workers.max(1).min(items.len());
    let mut chunks: Vec<Vec<T>> = (0..n).map(|_| Vec::new()).collect();
    for (i, item) in items.iter().enumerate() {
        chunks[i % n].push(item.clone());
    }
    chunks
}

fn visit_le(info: &DeviceInformation, seen: &mut HashSet<String>) -> Option<Device> {
    let id = info.Id().ok()?;
    let dev = op_result(BluetoothLEDevice::FromIdAsync(&id))?;

    let addr = format_addr(dev.BluetoothAddress().ok()?);
    if !seen.insert(addr.clone()) {
        return None;
    }

    let name = pick_name(info.Name(), dev.Name());
    let connected = dev.ConnectionStatus().ok()? == BluetoothConnectionStatus::Connected;

    let (battery, status) = if connected {
        match read_battery(&dev) {
            Some(b) => (Some(b), "ok"),
            None => match notify_cached(&addr) {
                Some(b) => (Some(b), "ok"),
                None => (None, "no-battery"),
            },
        }
    } else {
        (None, "offline")
    };
    let kind = classify_kind(&name);

    if connected {
        ensure_notify_sub(&addr, &dev);
    }

    Some(Device {
        name,
        address: addr,
        is_le: true,
        connected,
        battery,
        status,
        kind,
    })
}

fn notify_cached(addr: &str) -> Option<u8> {
    let cache = NOTIFY_CACHE.lock().ok()?;
    let (pct, at) = cache.get(addr)?;
    if at.elapsed() < NOTIFY_TTL {
        Some(*pct)
    } else {
        None
    }
}

fn visit_classic(info: &DeviceInformation, seen: &mut HashSet<String>) -> Option<Device> {
    let id = info.Id().ok()?;
    let dev = op_result(BluetoothDevice::FromIdAsync(&id))?;

    let addr = format_addr(dev.BluetoothAddress().ok()?);
    if !seen.insert(addr.clone()) {
        return None;
    }

    let name = pick_name(info.Name(), dev.Name());
    let connected = dev.ConnectionStatus().ok()? == BluetoothConnectionStatus::Connected;
    let kind = classify_kind(&name);

    Some(Device {
        name,
        address: addr,
        is_le: false,
        connected,
        battery: None,
        status: if connected { "no-battery" } else { "offline" },
        kind,
    })
}

fn pick_name(
    info_name: windows::core::Result<windows::core::HSTRING>,
    dev_name: windows::core::Result<windows::core::HSTRING>,
) -> String {
    if let Ok(n) = info_name {
        let s = n.to_string();
        if !s.trim().is_empty() {
            return s;
        }
    }
    dev_name.map(|n| n.to_string()).unwrap_or_default()
}

/// Read the Battery Level characteristic (read mode). Service/characteristic
/// discovery hits the system cache (they never change); only the value itself
/// goes to the device.
fn read_battery(dev: &BluetoothLEDevice) -> Option<u8> {
    let battery_svc = GattServiceUuids::Battery().ok()?;
    let svc_res = op_result(
        dev.GetGattServicesForUuidWithCacheModeAsync(battery_svc, BluetoothCacheMode::Cached),
    )?;
    if svc_res.Status().ok()? != GattCommunicationStatus::Success {
        return None;
    }
    let services = svc_res.Services().ok()?;
    if services.Size().ok()? == 0 {
        return None;
    }
    let svc = services.GetAt(0).ok()?;

    let battery_char = GattCharacteristicUuids::BatteryLevel().ok()?;
    let ch_res = op_result(
        svc.GetCharacteristicsForUuidWithCacheModeAsync(battery_char, BluetoothCacheMode::Cached),
    )?;
    if ch_res.Status().ok()? != GattCommunicationStatus::Success {
        return None;
    }
    let chars = ch_res.Characteristics().ok()?;
    if chars.Size().ok()? == 0 {
        return None;
    }
    let ch = chars.GetAt(0).ok()?;

    let read = op_result(ch.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached))?;
    if read.Status().ok()? != GattCommunicationStatus::Success {
        return None;
    }
    let buffer = read.Value().ok()?;
    battery_from_buffer(&buffer)
}

fn battery_from_buffer(buffer: &windows::Storage::Streams::IBuffer) -> Option<u8> {
    if buffer.Length().unwrap_or(0) == 0 {
        return None;
    }
    let reader = DataReader::FromBuffer(buffer).ok()?;
    let v = reader.ReadByte().ok()?;
    if v <= 100 { Some(v) } else { None }
}

/// Subscribe once per device to Battery Level notifications. Pushed values
/// update the strip immediately (see on_battery_notify) without a poll.
fn ensure_notify_sub(addr: &str, dev: &BluetoothLEDevice) {
    {
        let subs = NOTIFY_SUBS.lock().unwrap();
        if subs.contains_key(addr) {
            return;
        }
    }

    let battery_svc = match GattServiceUuids::Battery() {
        Ok(s) => s,
        Err(_) => return,
    };
    let Some(svc_res) = op_result(
        dev.GetGattServicesForUuidWithCacheModeAsync(battery_svc, BluetoothCacheMode::Cached),
    ) else {
        return;
    };
    let Ok(services) = svc_res.Services() else {
        return;
    };
    if services.Size().unwrap_or(0) == 0 {
        return;
    }
    let Ok(svc) = services.GetAt(0) else { return };
    let Ok(battery_char) = GattCharacteristicUuids::BatteryLevel() else {
        return;
    };
    let Some(ch_res) = op_result(
        svc.GetCharacteristicsForUuidWithCacheModeAsync(battery_char, BluetoothCacheMode::Cached),
    ) else {
        return;
    };
    let Ok(chars) = ch_res.Characteristics() else {
        return;
    };
    if chars.Size().unwrap_or(0) == 0 {
        return;
    }
    let Ok(ch) = chars.GetAt(0) else { return };
    let Ok(props) = ch.CharacteristicProperties() else {
        return;
    };
    if (props & GattCharacteristicProperties::Notify).0 == 0 {
        return;
    }

    let addr_owned = addr.to_string();
    let handler = TypedEventHandler::new(
        move |_ch: &Option<GattCharacteristic>, args: &Option<GattValueChangedEventArgs>| {
            let Some(args) = args else { return Ok(()) };
            if let Ok(buffer) = args.CharacteristicValue()
                && let Some(pct) = battery_from_buffer(&buffer)
            {
                on_battery_notify(&addr_owned, pct);
            }
            Ok(())
        },
    );
    let Ok(token) = ch.ValueChanged(&handler) else {
        return;
    };

    // Keep a GATT session so the stack does not tear the subscription down
    // when the device goes idle between polls.
    let session = dev
        .BluetoothDeviceId()
        .ok()
        .and_then(|id| op_result(GattSession::FromDeviceIdAsync(&id)));
    if let Some(s) = &session {
        let _ = s.SetMaintainConnection(true);
    }

    NOTIFY_SUBS.lock().unwrap().insert(
        addr.to_string(),
        NotifySub {
            _dev: dev.clone(),
            _session: session,
            _token: token,
        },
    );
    crate::dblog::log(&format!("notify sub {addr}"));
}

/// Called on a WinRT threadpool thread when a subscribed device pushes a value.
fn on_battery_notify(addr: &str, pct: u8) {
    if let Ok(mut cache) = NOTIFY_CACHE.lock() {
        cache.insert(addr.to_string(), (pct, Instant::now()));
    }
    if let Some(app) = crate::app::APP.get() {
        app.apply_notify_battery(addr, pct);
    }
}

/// Watch paired Bluetooth devices; a connect/disconnect kicks an immediate
/// refresh instead of waiting for the next poll tick.
pub fn start_connection_watcher() {
    {
        let watchers = WATCHERS.lock().unwrap();
        if !watchers.is_empty() {
            return;
        }
    }
    let mut created = Vec::new();
    for selector in [
        BluetoothLEDevice::GetDeviceSelectorFromPairingState(true),
        BluetoothDevice::GetDeviceSelectorFromPairingState(true),
    ] {
        let Ok(selector) = selector else { continue };
        let Ok(watcher) = DeviceInformation::CreateWatcherAqsFilter(&selector) else {
            continue;
        };
        let handler = TypedEventHandler::new(
            move |_sender: &Option<DeviceWatcher>, _update: &Option<DeviceInformationUpdate>| {
                kick_refresh();
                Ok(())
            },
        );
        let _ = watcher.Updated(&handler);
        let removed = TypedEventHandler::new(
            move |_sender: &Option<DeviceWatcher>, _update: &Option<DeviceInformationUpdate>| {
                kick_refresh();
                Ok(())
            },
        );
        let _ = watcher.Removed(&removed);
        let _ = watcher.Start();
        created.push(watcher);
    }
    if !created.is_empty() {
        crate::dblog::log(&format!("connection watchers started: {}", created.len()));
        *WATCHERS.lock().unwrap() = created;
    }
}

fn kick_refresh() {
    // Coalesce bursts of watcher events into at most one kick per 2s.
    let allow = {
        let mut last = LAST_KICK.lock().unwrap();
        if last.elapsed() >= Duration::from_secs(2) {
            *last = Instant::now();
            true
        } else {
            false
        }
    };
    if allow && let Some(app) = crate::app::APP.get() {
        app.request_refresh();
    }
}

fn format_addr(addr: u64) -> String {
    let v = addr & 0xFFFF_FFFF_FFFF;
    let hex = format!("{v:012X}");
    punctuate_bt_addr(&hex)
}

fn punctuate_bt_addr(hex12: &str) -> String {
    hex12
        .as_bytes()
        .chunks(2)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(":")
}

fn normalize_bt_addr(addr: &str) -> String {
    addr.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Pull the 12-digit MAC out of a BTHENUM instance id.
pub fn bt_addr_from_instance_id(id: &str) -> Option<String> {
    let up = id.to_ascii_uppercase().replace(':', "");
    if let Some(rest) = up.split("DEV_").nth(1) {
        let hex: String = rest.chars().take(12).collect();
        if hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(punctuate_bt_addr(&hex));
        }
    }
    if let Some(idx) = up.find("_C000") {
        if idx >= 12 {
            let hex = &up[idx - 12..idx];
            if hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(punctuate_bt_addr(&hex));
            }
        }
    }
    None
}

fn hfp_battery_map() -> HashMap<String, u8> {
    let mut map = HashMap::new();
    unsafe {
        let flags = SETUP_DI_GET_CLASS_DEVS_FLAGS(DIGCF_PRESENT.0 | DIGCF_ALLCLASSES.0);
        let Ok(hdev) = SetupDiGetClassDevsW(None, w!("BTHENUM"), HWND::default(), flags) else {
            return map;
        };
        let mut i = 0u32;
        loop {
            let mut info = SP_DEVINFO_DATA::default();
            info.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
            if SetupDiEnumDeviceInfo(hdev, i, &mut info).is_err() {
                break;
            }
            i += 1;
            let mut idbuf = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(hdev, &info, Some(&mut idbuf), None).is_err() {
                continue;
            }
            let end = idbuf.iter().position(|&c| c == 0).unwrap_or(idbuf.len());
            let inst = String::from_utf16_lossy(&idbuf[..end]);
            let Some(addr) = bt_addr_from_instance_id(&inst) else {
                continue;
            };
            let mut ptype = DEVPROPTYPE(0);
            let mut buf = [0u8; 8];
            if SetupDiGetDevicePropertyW(
                hdev,
                &info,
                &HFP_BATTERY_KEY,
                &mut ptype,
                Some(&mut buf),
                None,
                0,
            )
            .is_err()
            {
                continue;
            }
            if ptype != DEVPROP_TYPE_BYTE {
                continue;
            }
            let pct = buf[0];
            if pct <= 100 {
                map.insert(normalize_bt_addr(&addr), pct);
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(hdev);
    }
    map
}

fn classify_kind(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("keyboard") || n.starts_with("k3") || n.starts_with("k7") || n.starts_with("k8") {
        "Keyboard"
    } else if n.contains("mouse")
        || n.contains("mx ")
        || n.contains("g502")
        || n.contains("g102")
        || n.contains("viper")
        || n.contains("tail")
        || n.contains("m720")
    {
        "Mouse"
    } else if n.contains("headset")
        || n.contains("headphone")
        || n.contains("earbud")
        || n.contains("buds")
        || n.contains("airpods")
        || n.contains("w820")
        || n.contains("w800")
        || n.contains("pilot")
        || n.contains("wh-1000")
        || n.contains("xm5")
        || n.contains("xm4")
        || n.contains("sony")
    {
        "Headset"
    } else if n.contains("controller")
        || n.contains("gamepad")
        || n.contains("dualsense")
        || n.contains("dualshock")
        || n.contains("ps5")
        || n.contains("xbox")
        || n.contains("switch")
    {
        "Gamepad"
    } else if n.contains("pen") || n.contains("stylus") || n.contains("surface") {
        "Pen"
    } else if n.contains("trackpad") || n.contains("touchpad") {
        "Trackpad"
    } else {
        "Unknown"
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_handsfree_and_dev_instance_ids() {
        let hfp = r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&0002\8&129DF44A&0&0CAEBDBD05CF_C00000000";
        assert_eq!(
            bt_addr_from_instance_id(hfp).as_deref(),
            Some("0C:AE:BD:BD:05:CF")
        );
        let dev = r"BTHENUM\DEV_0CAEBDBD05CF\8&129DF44A&0&BLUETOOTHDEVICE_0CAEBDBD05CF";
        assert_eq!(
            bt_addr_from_instance_id(dev).as_deref(),
            Some("0C:AE:BD:BD:05:CF")
        );
        assert_eq!(normalize_bt_addr("0c:ae:bd:bd:05:cf"), "0CAEBDBD05CF");
    }

    #[test]
    fn split_for_workers_distributes_all_items() {
        let items: Vec<u32> = (0..7).collect();
        let chunks = split_for_workers(&items, 3);
        let mut all: Vec<u32> = chunks.iter().flatten().copied().collect();
        all.sort();
        assert_eq!(all, items);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn split_for_workers_empty_and_single() {
        let empty: Vec<u32> = Vec::new();
        assert!(split_for_workers(&empty, 3).is_empty());
        let one = split_for_workers(&[42u32], 4);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0], vec![42]);
    }
}
