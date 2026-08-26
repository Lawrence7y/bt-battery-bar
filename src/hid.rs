//! HID (2.4G/USB receiver) battery layer.
#![allow(unsafe_op_in_unsafe_fn)]
//!
//! Windows exposes 2.4G dongles as USB HID collections. The keyboard/mouse
//! collections are opened exclusively by kbdhid/mouhid, so we:
//!   1. open with R/W + FILE_FLAG_OVERLAPPED (hidapi pattern)
//!   2. fall back to access=0 so HidD_* still works on exclusive collections
//!   3. talk to the vendor collection (usage page 0xFF00+) where battery lives
//!
//! Compx / ATK / VXE / VGN dongles (VID 0x373B and several rebrands) answer a
//! 17-byte report-id 0x08 command 0x04 with the percentage in byte 6.

use std::collections::{HashMap, HashSet};
use std::mem::{size_of, zeroed};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SETUP_DI_GET_CLASS_DEVS_FLAGS, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_REPORT_TYPE, HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS,
    HidD_FlushQueue, HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid,
    HidD_GetManufacturerString, HidD_GetPreparsedData, HidD_GetProductString, HidD_SetFeature,
    HidD_SetNumInputBuffers, HidD_SetOutputReport, HidP_Feature, HidP_GetCaps, HidP_GetUsageValue,
    HidP_GetValueCaps, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{
    BOOLEAN, CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, HWND,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::PCWSTR;

use crate::model::Device;

const COMPX_REPORT_ID: u8 = 0x08;
const COMPX_REPORT_ID_ALT: u8 = 0x13;
const COMPX_CMD_BATTERY: u8 = 0x04;
const COMPX_PACKET: usize = 17;
const COMPX_PACKET_ALT: usize = 20;
const TRANSACT_MS: u32 = 1000;
const COMPX_TRIES: u32 = 2;
const CACHE_TTL: Duration = Duration::from_secs(30 * 60);
/// Sonix 2.4G status is pushed roughly every 30s. Keep the RF link up across
/// polls that consume the queued packet and then see an empty HID buffer.
const SONIX_LINK_TTL: Duration = Duration::from_secs(90);

static BATTERY_CACHE: LazyLock<Mutex<HashMap<(u16, u16), (u8, Instant)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static LIVE_LINKS: LazyLock<Mutex<HashMap<(u16, u16), LiveLink>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static HAD_X87_WIRED: AtomicBool = AtomicBool::new(false);

/// Per-receiver RF/wired link remembered across HID polls and the watcher.
#[derive(Clone, Debug, Default)]
pub struct LiveLink {
    pub battery: Option<u8>,
    last_rf: Option<Instant>,
    pub wired: bool,
}

impl LiveLink {
    pub fn on_battery(&mut self, pct: u8, at: Instant) {
        self.battery = Some(pct);
        self.last_rf = Some(at);
    }

    pub fn touch_rf(&mut self, at: Instant) {
        self.last_rf = Some(at);
    }

    pub fn set_wired(&mut self, present: bool) {
        self.wired = present;
    }

    pub fn connected(&self, now: Instant, ttl: Duration) -> bool {
        if self.wired {
            return true;
        }
        match self.last_rf {
            Some(at) => now.checked_duration_since(at).is_some_and(|dt| dt < ttl),
            None => false,
        }
    }
}

pub(crate) fn note_live_battery(vid: u16, pid: u16, pct: u8) {
    let now = Instant::now();
    if let Ok(mut map) = LIVE_LINKS.lock() {
        map.entry((vid, pid)).or_default().on_battery(pct, now);
        if vid == 0x0C45 && (pid == 0xFEFE || pid == 0x8006) {
            map.entry((0x0C45, 0xFEFE))
                .or_default()
                .on_battery(pct, now);
            map.entry((0x0C45, 0x8006))
                .or_default()
                .on_battery(pct, now);
        }
    }
    if let Ok(mut cache) = BATTERY_CACHE.lock() {
        cache.insert((vid, pid), (pct, now));
        if vid == 0x0C45 && (pid == 0xFEFE || pid == 0x8006) {
            cache.insert((0x0C45, 0xFEFE), (pct, now));
        }
    }
}

pub(crate) fn overlay_live(raw: &mut [HidDevice]) {
    let Ok(map) = LIVE_LINKS.lock() else {
        return;
    };
    let now = Instant::now();
    for d in raw {
        let Some(link) = map.get(&(d.vid, d.pid)) else {
            continue;
        };
        if link.connected(now, SONIX_LINK_TTL) {
            d.link_up = true;
            if d.battery.is_none() {
                d.battery = link.battery;
            }
        }
    }
}

const KNOWN_24G_VIDS: &[u16] = &[
    0x373B, // Compx / ATK / many Chinese 2.4G dongles
    0x046D, // Logitech
    0x3554, // VXE
    0x258A, // Glorious / similar
    0x1532, // Razer
    0x1038, // SteelSeries
    0x0951, // Kingston / HyperX
    0x3434, // Keychron
    0x1EA7, // Sharkoon / various
    0x248A, // Maxxter / various
    0x391D, // VGN
];

/// A successfully opened HID top-level collection.
#[derive(Clone, Debug)]
pub struct HidDevice {
    pub path: String,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: u16,
    pub output_len: u16,
    pub feature_len: u16,
    pub product: String,
    pub manufacturer: String,
    pub access: &'static str,
    /// Battery found via standard HID usage or vendor protocol, 0..=100.
    pub battery: Option<u8>,
    /// First successful feature/vendor report bytes (hex) for debugging.
    pub feature_hex: String,
    /// Why a vendor battery query failed (empty if unused).
    pub debug: String,
    /// True when this collection proves the peripheral is actually on-link
    /// (wired USB present, or a fresh 2.4G / VIA reply — not a leftover HID buffer).
    pub link_up: bool,
}

/// Returns (opened devices, list of failures encountered).
pub fn enumerate() -> (Vec<HidDevice>, Vec<String>) {
    enumerate_ex(true)
}

pub(crate) fn enumerate_ex(query: bool) -> (Vec<HidDevice>, Vec<String>) {
    let mut out = Vec::new();
    let mut fails = Vec::new();
    for path in list_hid_paths() {
        match probe_one(&path, query) {
            Ok(Some(d)) => out.push(d),
            Ok(None) => {}
            Err(e) => fails.push(format!("{path}: {e}")),
        }
    }
    (out, fails)
}

/// HID interface paths currently present. Does not open any device.
pub(crate) fn list_hid_paths() -> Vec<String> {
    let mut paths = Vec::new();
    unsafe {
        let guid = HidD_GetHidGuid();
        let flags = SETUP_DI_GET_CLASS_DEVS_FLAGS(DIGCF_PRESENT.0 | DIGCF_DEVICEINTERFACE.0);
        let Ok(hdev) = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), HWND::default(), flags)
        else {
            return paths;
        };
        let mut i: u32 = 0;
        loop {
            let mut ifdata = SP_DEVICE_INTERFACE_DATA::default();
            ifdata.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(hdev, None, &guid, i, &mut ifdata).is_err() {
                break;
            }
            i += 1;
            let mut req = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(hdev, &ifdata, None, 0, Some(&mut req), None);
            if req == 0 {
                continue;
            }
            let mut detail: Vec<u8> = vec![0u8; req as usize];
            let pdetail = detail.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*pdetail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(hdev, &ifdata, Some(pdetail), req, None, None)
                .is_err()
            {
                continue;
            }
            let path = read_detail_path(&detail);
            if !path.is_empty() {
                paths.push(path);
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(hdev);
    }
    paths
}

/// Convert present 2.4G / HID peripherals into the UI device list.
pub fn enumerate_devices() -> Vec<Device> {
    devices_from_raw(enumerate().0, true)
}

pub(crate) fn devices_from_raw(mut raw: Vec<HidDevice>, boost: bool) -> Vec<Device> {
    let has_wired = raw.iter().any(|d| is_sonix_vid(d.vid) && d.pid == 0x8006);
    if boost {
        boost_sonix_if_needed(&mut raw, has_wired);
    }
    overlay_live(&mut raw);
    let mut devs = collections_to_devices(&raw);
    apply_x87_mode_transition(&mut devs, has_wired);
    apply_battery_cache(&mut devs);
    devs
}

/// After USB cable unplug, the dongle stays present with an empty HID queue
/// until the next periodic 0x0F. Treat the unplug as switching to 2.4G.
fn apply_x87_mode_transition(devs: &mut [Device], has_wired: bool) {
    if let Ok(mut map) = LIVE_LINKS.lock() {
        map.entry((0x0C45, 0x8006))
            .or_default()
            .set_wired(has_wired);
    }
    let had_wired = HAD_X87_WIRED.swap(has_wired, Ordering::SeqCst);
    if has_wired {
        return;
    }
    if had_wired {
        let _ = sonix_rf_alive(0x0C45, 0xFEFE, true);
    }
    if had_wired || sonix_rf_alive(0x0C45, 0xFEFE, false) {
        for d in devs.iter_mut() {
            if d.address.to_ascii_uppercase().contains("0C45:FEFE") {
                d.connected = true;
                d.status = "ok";
            }
        }
    }
}

/// When the cable is gone and the first pass saw no 0x0F, wait for an unsolicited
/// RF status (the dongle only pushes every few tens of seconds).
fn boost_sonix_if_needed(raw: &mut [HidDevice], has_wired: bool) {
    if has_wired {
        return;
    }
    for d in raw.iter_mut() {
        if d.vid != 0x0C45 || d.pid != 0xFEFE || d.input_len != 65 {
            continue;
        }
        if d.link_up || d.battery.is_some() {
            continue;
        }
        if let Some((pct, hex)) = wait_sonix_packet(&d.path, d.input_len, 3500) {
            d.battery = Some(pct);
            d.feature_hex = hex;
            d.link_up = true;
            d.debug.clear();
            let _ = sonix_rf_alive(d.vid, d.pid, true);
        }
    }
}

fn wait_sonix_packet(path: &str, input_len: u16, timeout_ms: u32) -> Option<(u8, String)> {
    unsafe {
        let (handle, _) = open_hid(path)?;
        let buf = overlapped_read(handle, (input_len as usize).max(5), timeout_ms).ok();
        let _ = CloseHandle(handle);
        buf.and_then(|b| parse_sonix_battery(&b).map(|pct| (pct, to_hex(&b))))
    }
}

fn apply_battery_cache(devs: &mut [Device]) {
    let Ok(mut cache) = BATTERY_CACHE.lock() else {
        return;
    };
    for d in devs.iter_mut() {
        // 断开的设备不保留电量信息（显示 "--"）。
        if !d.connected {
            d.battery = None;
            continue;
        }
        let Some((vid, pid)) = parse_hid_addr(&d.address) else {
            continue;
        };
        if let Some(pct) = d.battery {
            cache.insert((vid, pid), (pct, Instant::now()));
        } else if let Some((pct, at)) = cache.get(&(vid, pid)) {
            // 已连接但本轮查询失败：用 30 分钟内的最后已知值补上。
            if at.elapsed() < CACHE_TTL {
                d.battery = Some(*pct);
            }
        }
    }
}

fn parse_hid_addr(addr: &str) -> Option<(u16, u16)> {
    // hid:373B:101B:Mouse
    let mut parts = addr.split(':');
    if parts.next()? != "hid" {
        return None;
    }
    let vid = u16::from_str_radix(parts.next()?, 16).ok()?;
    let pid = u16::from_str_radix(parts.next()?, 16).ok()?;
    Some((vid, pid))
}

pub fn vid_pid_from_hid_path(path: &str) -> Option<(u16, u16)> {
    let u = path.to_ascii_uppercase();
    let vid = u16::from_str_radix(after_tag(&u, "VID_")?, 16).ok()?;
    let pid = u16::from_str_radix(after_tag(&u, "PID_")?, 16).ok()?;
    Some((vid, pid))
}

fn after_tag<'a>(s: &'a str, tag: &str) -> Option<&'a str> {
    let rest = s.split(tag).nth(1)?;
    rest.get(..4)
        .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn hid_path_is_x87_24g(path: &str) -> bool {
    matches!(vid_pid_from_hid_path(path), Some((0x0C45, 0xFEFE)))
}

/// Keystrokes on the 2.4G dongle. Returns true if this transitioned the link to up.
pub fn note_x87_key_activity() -> bool {
    let was = sonix_rf_alive(0x0C45, 0xFEFE, false);
    let _ = sonix_rf_alive(0x0C45, 0xFEFE, true);
    !was
}

/// Live HID write/read experiment on Compx (mouse) and Sonix (keyboard) vendor collections.
pub fn spy() {
    let (devs, _) = enumerate();
    for d in &devs {
        if d.access != "R/W" || !is_vendor_page(d.usage_page) {
            continue;
        }
        if d.vid != 0x373B && d.vid != 0x0C45 {
            continue;
        }
        println!(
            "\n--- spy {:04X}:{:04X} uP={:04X} u={:04X} in={} out={} feat={} prod={} ---",
            d.vid,
            d.pid,
            d.usage_page,
            d.usage,
            d.input_len,
            d.output_len,
            d.feature_len,
            d.product
        );
        unsafe {
            let in_len = d.input_len as usize;
            let out_len = d.output_len as usize;
            if in_len == 0 {
                println!("  skip (no input report)");
                continue;
            }

            if d.vid == 0x373B && in_len == 17 {
                if let Some((h0, _)) = open_hid(&d.path) {
                    let _ = HidD_SetNumInputBuffers(h0, 64);
                    let cmd = pad_report(build_compx_cmd(0x08, 17), out_len.max(17));
                    match hid_write_then_read(h0, &cmd, in_len, 1500, true) {
                        Ok(buf) => println!("  fresh-set-then-read -> {}", to_hex(&buf)),
                        Err(e) => println!("  fresh-set-then-read -> {e}"),
                    }
                    let _ = CloseHandle(h0);
                }
            }

            let Some((handle, _)) = open_hid(&d.path) else {
                println!("  open failed");
                continue;
            };
            let _ = HidD_SetNumInputBuffers(handle, 64);

            let mut cmds: Vec<(String, Vec<u8>)> = Vec::new();
            if in_len == 17 || out_len == 17 {
                cmds.push((
                    "compx-08-04".into(),
                    pad_report(build_compx_cmd(0x08, 17), out_len.max(17)),
                ));
            }
            if in_len == 20 || out_len == 20 {
                cmds.push((
                    "compx-13-04".into(),
                    pad_report(build_compx_cmd(0x13, 20), out_len.max(20)),
                ));
            }
            if in_len == 49 || out_len == 49 {
                let mut rid0 = vec![0u8; out_len.max(49)];
                rid0[1] = 0x04;
                cmds.push(("rid0-cmd04".into(), rid0));
                cmds.push((
                    "compx-49".into(),
                    pad_report(build_compx_cmd(0x08, 49), out_len.max(49)),
                ));
            }
            if in_len == 33 || out_len == 33 {
                cmds.push(("via-proto".into(), qmk_cmd(out_len.max(33), 0x01, &[])));
                cmds.push(("qmk-a4".into(), qmk_cmd(out_len.max(33), 0xA4, &[])));
                cmds.push((
                    "via-custom".into(),
                    qmk_cmd(out_len.max(33), 0x08, &[0x00, 0x00]),
                ));
                cmds.push(("via-kbval".into(), qmk_cmd(out_len.max(33), 0x02, &[0x01])));
            }
            if in_len == 65 || out_len == 65 {
                cmds.push(("via65-proto".into(), qmk_cmd(out_len.max(65), 0x01, &[])));
                cmds.push(("qmk65-a4".into(), qmk_cmd(out_len.max(65), 0xA4, &[])));
                cmds.push(("cmd-0f".into(), {
                    let mut r = vec![0u8; out_len.max(65)];
                    r[1] = 0x0F;
                    r
                }));
                cmds.push(("cmd-04".into(), {
                    let mut r = vec![0u8; out_len.max(65)];
                    r[1] = 0x04;
                    r
                }));
                cmds.push(("unique-99".into(), {
                    let mut r = vec![0u8; out_len.max(65)];
                    r[1] = 0x99;
                    r[2] = 0x88;
                    r[3] = 0x77;
                    r
                }));
            }
            if in_len == 33 || out_len == 33 {
                cmds.push((
                    "via-custom-ch5".into(),
                    qmk_cmd(out_len.max(33), 0x08, &[0x05, 0x01]),
                ));
                cmds.push((
                    "via-custom-ch0".into(),
                    qmk_cmd(out_len.max(33), 0x08, &[0x00, 0x01]),
                ));
            }
            if in_len == 49 || out_len == 49 {
                cmds.push((
                    "compx-49-set".into(),
                    pad_report(build_compx_cmd(0x08, 49), out_len.max(49)),
                ));
            }

            for (name, cmd) in cmds {
                match hid_transact(handle, &cmd, in_len.max(cmd.len()), 1200) {
                    Ok(buf) => println!("  {name} -> {}", to_hex(&buf)),
                    Err(e) => println!("  {name} -> {e}"),
                }
            }

            if d.vid == 0x373B && in_len == 17 {
                let cmd = pad_report(build_compx_cmd(0x08, 17), out_len.max(17));
                match hid_write_then_read(handle, &cmd, in_len, 1000, true) {
                    Ok(buf) => println!("  set-then-read -> {}", to_hex(&buf)),
                    Err(e) => println!("  set-then-read -> {e}"),
                }
                match hid_write_then_read(handle, &cmd, in_len, 1000, false) {
                    Ok(buf) => println!("  write-then-read -> {}", to_hex(&buf)),
                    Err(e) => println!("  write-then-read -> {e}"),
                }
                let eeprom = {
                    let mut r = vec![0u8; out_len.max(17)];
                    r[0] = 0x08;
                    r[1] = 0x08;
                    r[5] = 0x01;
                    let last = 16.min(r.len() - 1);
                    r[last] = compx_checksum(&r[..last]);
                    r
                };
                match hid_write_then_read(handle, &eeprom, in_len, 1000, true) {
                    Ok(buf) => println!("  eeprom-set-then-read -> {}", to_hex(&buf)),
                    Err(e) => println!("  eeprom-set-then-read -> {e}"),
                }
            }
            if d.vid == 0x0C45 && in_len == 65 {
                match overlapped_read(handle, in_len, 2500) {
                    Ok(buf) => println!("  long-unsolicited -> {}", to_hex(&buf)),
                    Err(e) => println!("  long-unsolicited -> {e}"),
                }
            }
            let _ = CloseHandle(handle);
        }
    }
}

/// Enumerate QMK VIA (FF60) command/channel/value space on a 前行者 dongle
/// looking for a live battery source. Debug helper: `bt-battery-bar via`.
pub fn via_probe() {
    let (devs, _) = enumerate();
    for d in &devs {
        if d.vid != 0x0C45 || d.usage_page != 0xFF60 || (d.input_len != 33 && d.input_len != 65) {
            continue;
        }
        println!(
            "--- via-probe {:04X}:{:04X} uP={:04X} in={} out={} ---",
            d.vid, d.pid, d.usage_page, d.input_len, d.output_len
        );
        unsafe {
            let Some((handle, _)) = open_hid(&d.path) else {
                println!("  open failed");
                continue;
            };
            let _ = HidD_SetNumInputBuffers(handle, 64);
            let in_len = (d.input_len as usize).max(2);
            let out_len = (d.output_len as usize).max(33);

            let mut cmds: Vec<(String, Vec<u8>)> = Vec::new();
            cmds.push(("proto-01".into(), qmk_cmd(out_len, 0x01, &[])));
            for id in 0u8..0x10 {
                cmds.push((format!("kbval-02-{id:02X}"), qmk_cmd(out_len, 0x02, &[id])));
            }
            for ch in 0u8..0x10 {
                cmds.push((
                    format!("custom-08-ch{ch:02X}"),
                    qmk_cmd(out_len, 0x08, &[ch, 0x00]),
                ));
            }
            for ch in 0u8..0x08 {
                cmds.push((
                    format!("custom-08-ch{ch:02X}-v01"),
                    qmk_cmd(out_len, 0x08, &[ch, 0x01]),
                ));
            }
            cmds.push(("qmk-a4".into(), qmk_cmd(out_len, 0xA4, &[])));
            cmds.push(("q20".into(), qmk_cmd(out_len, 0x20, &[0x01, 0x00])));
            cmds.push(("q20b".into(), qmk_cmd(out_len, 0x20, &[])));

            for (name, cmd) in cmds {
                let _ = HidD_FlushQueue(handle);
                if overlapped_write(handle, &cmd).is_err() {
                    println!("  {name} -> write failed");
                    continue;
                }
                let mut printed = 0usize;
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1600);
                while printed < 3 && std::time::Instant::now() < deadline {
                    let remain = deadline.saturating_duration_since(std::time::Instant::now());
                    let ms = remain.as_millis().min(800).max(1) as u32;
                    match overlapped_read(handle, in_len, ms) {
                        Ok(buf) => {
                            if buf.iter().any(|&b| b != 0) {
                                println!("  {name} -> {}", to_hex(&buf));
                                printed += 1;
                            }
                        }
                        Err(_) => break,
                    }
                }
                if printed == 0 {
                    println!("  {name} -> (no reply)");
                }
            }
            let _ = CloseHandle(handle);
        }
    }
}

/// Probe HidD_GetFeature / SetFeature+GetFeature on the vendor interfaces
/// (the official EWEADN driver uses feature reports on MI_03/FF60).
pub fn feat_probe() {
    let (devs, _) = enumerate_ex(false);
    for d in &devs {
        if d.vid != 0x0C45 || d.pid != 0xFEFE || d.access != "R/W" || d.feature_len == 0 {
            continue;
        }
        println!(
            "--- feat-probe uP={:04X} in={} out={} feat={} ---",
            d.usage_page, d.input_len, d.output_len, d.feature_len
        );
        unsafe {
            let Some((handle, _)) = open_hid(&d.path) else {
                println!("  open failed");
                continue;
            };
            let feat_len = (d.feature_len as usize).max(2);
            for rid in 0u8..=0x20u8 {
                let mut buf = vec![0u8; feat_len];
                buf[0] = rid;
                let ok = hid_boolean_ok(HidD_GetFeature(
                    handle,
                    buf.as_mut_ptr() as *mut _,
                    buf.len() as u32,
                ));
                if ok && buf.iter().skip(1).any(|&b| b != 0) {
                    println!("  GetFeature {rid:02X} -> {}", to_hex(&buf));
                }
            }
            // SetFeature(x) then GetFeature(y) round-trips on FF60.
            if d.usage_page == 0xFF60 {
                for rid in 0u8..=0x10u8 {
                    let mut set = vec![0u8; feat_len];
                    set[0] = rid;
                    let _ = hid_boolean_ok(HidD_SetFeature(
                        handle,
                        set.as_ptr() as *const _,
                        set.len() as u32,
                    ));
                    let mut get = vec![0u8; feat_len];
                    get[0] = rid;
                    let ok = hid_boolean_ok(HidD_GetFeature(
                        handle,
                        get.as_mut_ptr() as *mut _,
                        get.len() as u32,
                    ));
                    if ok && get.iter().skip(1).any(|&b| b != 0) {
                        println!("  Set+Get {rid:02X} -> {}", to_hex(&get));
                    }
                }
            }
            println!("  done");
            let _ = CloseHandle(handle);
        }
    }
}

/// Brute the FF59 command space: any response that differs from the known
/// static heartbeat (`00 0F 01 01 11 ...`) is a candidate live-data command.
pub fn scan2() {
    let (devs, _) = enumerate_ex(false);
    for d in &devs {
        if d.vid != 0x0C45 || d.pid != 0xFEFE || d.usage_page != 0xFF59 || d.output_len == 0 {
            continue;
        }
        println!(
            "--- scan2 uP={:04X} in={} out={} ---",
            d.usage_page, d.input_len, d.output_len
        );
        unsafe {
            let Some((handle, _)) = open_hid(&d.path) else {
                println!("  open failed");
                continue;
            };
            let _ = HidD_SetNumInputBuffers(handle, 64);
            let in_len = (d.input_len as usize).max(1);
            let out_len = d.output_len as usize;
            let heartbeat: Vec<Vec<u8>> = vec![vec![0x00, 0x0F, 0x01, 0x01, 0x11]];
            let is_known = |buf: &[u8]| {
                heartbeat
                    .iter()
                    .any(|h| buf.len() >= h.len() && buf[..h.len()] == h[..])
            };
            let mut interesting = 0usize;
            for b1 in 0u8..=0xFF {
                let mut pkt = vec![0u8; out_len];
                pkt[1] = b1;
                if overlapped_write(handle, &pkt).is_err() {
                    continue;
                }
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(260);
                while std::time::Instant::now() < deadline {
                    match overlapped_read(handle, in_len, 60) {
                        Ok(buf) => {
                            if !buf.iter().all(|&b| b == 0) && !is_known(&buf) {
                                println!("  cmd[1]={b1:02X} -> {}", to_hex(&buf));
                                interesting += 1;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            for b2 in 0u8..=0xFF {
                let mut pkt = vec![0u8; out_len];
                pkt[1] = 0x0F;
                pkt[2] = b2;
                if overlapped_write(handle, &pkt).is_err() {
                    continue;
                }
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(260);
                while std::time::Instant::now() < deadline {
                    match overlapped_read(handle, in_len, 60) {
                        Ok(buf) => {
                            if !buf.iter().all(|&b| b == 0) && !is_known(&buf) {
                                println!("  cmd 0F {:02X} -> {}", b2, to_hex(&buf));
                                interesting += 1;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            println!("  done, interesting responses: {interesting}");
            let _ = CloseHandle(handle);
        }
    }
}

/// Brute a live 前行者 2.4G dongle for any command that returns data.
pub fn scan() {
    let (devs, _) = enumerate_ex(false);
    println!("=== empty listen 8s, then cmd brute on 0C45:FEFE vendor ===");
    for d in &devs {
        if d.vid != 0x0C45 || d.pid != 0xFEFE || d.access != "R/W" {
            continue;
        }
        println!(
            "\n--- {:04X} in={} out={} feat={} ---",
            d.usage_page, d.input_len, d.output_len, d.feature_len
        );
        unsafe {
            let Some((handle, _)) = open_hid(&d.path) else {
                println!("  open failed");
                continue;
            };
            let _ = HidD_SetNumInputBuffers(handle, 64);
            let in_len = (d.input_len as usize).max(1);
            let listen_ms = if d.input_len == 65 { 800 } else { 200 };
            match overlapped_read(handle, in_len, listen_ms) {
                Ok(buf) => println!("  unsolicited {}", to_hex(&buf)),
                Err(e) => println!("  unsolicited {e}"),
            }
            if d.output_len == 0 {
                let _ = CloseHandle(handle);
                continue;
            }
            if d.usage_page == 0xFF59 && d.output_len >= 2 {
                let mut pkt = vec![0u8; d.output_len as usize];
                pkt[1] = 0x0F;
                let set_ok = hid_boolean_ok(HidD_SetOutputReport(
                    handle,
                    pkt.as_ptr() as *const _,
                    pkt.len() as u32,
                ));
                println!("  SetOutput 0x0F ok={set_ok}");
                match overlapped_write(handle, &pkt) {
                    Ok(()) => println!("  WriteFile 0x0F ok"),
                    Err(e) => println!("  WriteFile 0x0F {e}"),
                }
                match overlapped_read(handle, in_len, 5000) {
                    Ok(buf) => println!("  after-0f {}", to_hex(&buf)),
                    Err(e) => println!("  after-0f {e}"),
                }
                for (tag, pkt) in [
                    ("aa55-0f", {
                        let mut p = vec![0u8; d.output_len as usize];
                        p[0] = 0xAA;
                        p[1] = 0x55;
                        p[2] = 0x0F;
                        p
                    }),
                    ("aafa-03", {
                        let mut p = vec![0u8; d.output_len as usize];
                        p[0] = 0xAA;
                        p[1] = 0xFA;
                        p[2] = 0x03;
                        p
                    }),
                    ("via-a4-b0", {
                        let mut p = vec![0u8; d.output_len as usize];
                        p[0] = 0xA4;
                        p
                    }),
                    ("cmd-a4", qmk_cmd(d.output_len as usize, 0xA4, &[])),
                ] {
                    match hid_write_then_read(handle, &pkt, in_len, 1500, false) {
                        Ok(buf) => println!("  {tag} {}", to_hex(&buf)),
                        Err(e) => println!("  {tag} {e}"),
                    }
                }
            }
            if d.usage_page == 0xFF60 && d.output_len >= 2 {
                for (tag, pkt) in [
                    ("via01-write", qmk_cmd(d.output_len as usize, 0x01, &[])),
                    ("via-a4-write", qmk_cmd(d.output_len as usize, 0xA4, &[])),
                    ("a4-at0", {
                        let mut p = vec![0u8; d.output_len as usize];
                        p[0] = 0xA4;
                        p
                    }),
                    ("01-at0", {
                        let mut p = vec![0u8; d.output_len as usize];
                        p[0] = 0x01;
                        p
                    }),
                ] {
                    match hid_write_then_read(handle, &pkt, in_len, 1500, false) {
                        Ok(buf) => println!("  {tag} {}", to_hex(&buf)),
                        Err(e) => println!("  {tag} {e}"),
                    }
                }
            }
            if d.feature_len > 0 {
                for id in 0u8..=16 {
                    let mut r = vec![0u8; d.feature_len as usize];
                    r[0] = id;
                    if hid_boolean_ok(HidD_GetFeature(
                        handle,
                        r.as_mut_ptr() as *mut _,
                        r.len() as u32,
                    )) && r.iter().any(|&b| b != 0 && b != id)
                    {
                        println!("  feature id={id:02X} {}", to_hex(&r));
                    }
                }
            }
            if false && d.output_len > 0 {
                let out_len = d.output_len as usize;
                for cmd in 0u8..=0x20 {
                    let mut pkt = vec![0u8; out_len];
                    pkt[1] = cmd;
                    match hid_write_then_read(handle, &pkt, in_len, 280, true) {
                        Ok(buf) => {
                            if parse_sonix_battery(&buf).is_some() || buf.iter().any(|&b| b != 0) {
                                println!(
                                    "  cmd[1]={cmd:02X} batt={:?} {}",
                                    parse_sonix_battery(&buf),
                                    to_hex(&buf)
                                );
                            }
                        }
                        Err(_) => {}
                    }
                }
                if out_len >= 1 {
                    for cmd in 0u8..=0x10 {
                        let mut pkt = vec![0u8; out_len];
                        pkt[0] = cmd;
                        match hid_write_then_read(handle, &pkt, in_len, 280, true) {
                            Ok(buf) => {
                                if buf.iter().any(|&b| b != 0) {
                                    println!(
                                        "  rid={cmd:02X} batt={:?} {}",
                                        parse_sonix_battery(&buf),
                                        to_hex(&buf)
                                    );
                                }
                            }
                            Err(_) => {}
                        }
                    }
                }
            }
            let _ = CloseHandle(handle);
        }
    }
}

fn pad_report(mut cmd: Vec<u8>, len: usize) -> Vec<u8> {
    if cmd.len() < len {
        cmd.resize(len, 0);
    }
    cmd
}

fn qmk_cmd(total_len: usize, cmd: u8, extra: &[u8]) -> Vec<u8> {
    let mut r = vec![0u8; total_len.max(2)];
    r[0] = 0; // report id
    r[1] = cmd;
    for (i, b) in extra.iter().enumerate() {
        if 2 + i < r.len() {
            r[2 + i] = *b;
        }
    }
    r
}

unsafe fn hid_write_then_read(
    handle: HANDLE,
    out_buf: &[u8],
    in_len: usize,
    timeout_ms: u32,
    prefer_set_report: bool,
) -> Result<Vec<u8>, String> {
    let write_ok = if prefer_set_report {
        hid_boolean_ok(HidD_SetOutputReport(
            handle,
            out_buf.as_ptr() as *const _,
            out_buf.len() as u32,
        )) || overlapped_write(handle, out_buf).is_ok()
    } else {
        overlapped_write(handle, out_buf).is_ok()
            || hid_boolean_ok(HidD_SetOutputReport(
                handle,
                out_buf.as_ptr() as *const _,
                out_buf.len() as u32,
            ))
    };
    if !write_ok {
        return Err("write failed".into());
    }
    overlapped_read(handle, in_len.max(1), timeout_ms)
}

unsafe fn hid_transact(
    handle: HANDLE,
    out_buf: &[u8],
    in_len: usize,
    timeout_ms: u32,
) -> Result<Vec<u8>, String> {
    let event = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| e.to_string())?;
    let mut ov = OVERLAPPED::default();
    ov.hEvent = event;
    let mut buf = vec![0u8; in_len.max(1)];
    let read_res = ReadFile(
        handle,
        Some(buf.as_mut_slice()),
        None,
        Some(&mut ov as *mut _),
    );
    let pending = match read_res {
        Ok(()) => false,
        Err(e) if is_io_pending(&e) => true,
        Err(e) => {
            let _ = CloseHandle(event);
            return Err(format!("ReadFile arm 0x{:08X}", e.code().0 as u32));
        }
    };

    let write_ok = overlapped_write(handle, out_buf).is_ok()
        || hid_boolean_ok(HidD_SetOutputReport(
            handle,
            out_buf.as_ptr() as *const _,
            out_buf.len() as u32,
        ));
    if !write_ok {
        if pending {
            let _ = CancelIoEx(handle, Some(&ov as *const _));
            let mut n = 0u32;
            let _ = GetOverlappedResult(handle, &ov, &mut n, true);
        }
        let _ = CloseHandle(event);
        return Err("write failed".into());
    }

    let ok = if pending {
        wait_overlapped(handle, &ov, timeout_ms)
    } else {
        true
    };
    let mut n = 0u32;
    if ok {
        let _ = GetOverlappedResult(handle, &ov, &mut n, false);
    } else {
        let _ = CancelIoEx(handle, Some(&ov as *const _));
        let _ = GetOverlappedResult(handle, &ov, &mut n, true);
    }
    let _ = CloseHandle(event);
    if !ok {
        return Err("read timeout".into());
    }
    buf.truncate(n as usize);
    Ok(buf)
}

pub fn hid_boolean_ok(b: BOOLEAN) -> bool {
    b.0 != 0
}

pub fn hidp_success(status: i32) -> bool {
    status == HIDP_STATUS_SUCCESS.0
}

pub fn is_mouse_usage(page: u16, usage: u16) -> bool {
    page == 0x0001 && usage == 0x0002
}

pub fn is_keyboard_usage(page: u16, usage: u16) -> bool {
    page == 0x0001 && (usage == 0x0006 || usage == 0x0007)
}

pub fn is_vendor_page(page: u16) -> bool {
    page >= 0xFF00
}

pub fn is_compx_vid(vid: u16) -> bool {
    matches!(vid, 0x373B | 0x3554 | 0x391D)
}

pub fn is_sonix_vid(vid: u16) -> bool {
    vid == 0x0C45
}

pub fn is_known_24g_vid(vid: u16) -> bool {
    KNOWN_24G_VIDS.contains(&vid)
}

pub fn looks_like_24g_product(product: &str) -> bool {
    let p = product.to_lowercase();
    p.contains("2.4")
        || p.contains("dongle")
        || p.contains("receiver")
        || p.contains("unifying")
        || p.contains("lightspeed")
}

pub fn skip_noise(product: &str, path: &str, members: &[(u16, u16)]) -> bool {
    let p = product.to_lowercase();
    let h = path.to_lowercase();
    if p.contains("virtual")
        || p.contains("hidi2c")
        || p.contains("camera")
        || p.contains("webcam")
        || p.contains("audio")
    {
        return true;
    }
    if h.contains("gvinput") || h.contains("vhf") || h.contains("hid_device_system") {
        return true;
    }
    members.iter().any(|&(page, _)| page == 0x000D)
}

pub fn include_receiver(
    vid: u16,
    pid: u16,
    product: &str,
    path: &str,
    members: &[(u16, u16)],
) -> bool {
    if members.is_empty() || skip_noise(product, path, members) {
        return false;
    }
    // Sonix VID 0x0C45 is used by both the 前行者 X87 2.4G dongle (PID FEFE)
    // and the same keyboard on USB cable (PID 8006, product "X87").
    if is_sonix_vid(vid) {
        return pid == 0xFEFE || pid == 0x8006 || looks_like_24g_product(product);
    }
    if is_known_24g_vid(vid) || looks_like_24g_product(product) {
        return true;
    }
    let has_mk = members
        .iter()
        .any(|&(p, u)| is_mouse_usage(p, u) || is_keyboard_usage(p, u));
    let has_vendor = members.iter().any(|&(p, _)| is_vendor_page(p));
    has_mk && has_vendor
}

pub fn receiver_kinds(members: &[(u16, u16)]) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    if members.iter().any(|&(p, u)| is_mouse_usage(p, u)) {
        kinds.push("Mouse");
    }
    if members.iter().any(|&(p, u)| is_keyboard_usage(p, u)) {
        kinds.push("Keyboard");
    }
    if kinds.is_empty() {
        kinds.push("Unknown");
    }
    kinds
}

pub fn friendly_product(vid: u16, pid: u16, product: &str) -> String {
    match (vid, pid) {
        (0x0C45, 0xFEFE) => "前行者 X87 2.4G".into(),
        (0x0C45, 0x8006) => "前行者 X87 有线".into(),
        _ => product.trim().to_string(),
    }
}

#[allow(dead_code)]
pub fn display_name(product: &str, kind: &str) -> String {
    display_name_for(0, 0, product, kind)
}

pub fn display_name_for(vid: u16, pid: u16, product: &str, kind: &str) -> String {
    let p = friendly_product(vid, pid, product);
    let kind_cn = match kind {
        "Keyboard" => "键盘",
        "Mouse" => "鼠标",
        _ => "设备",
    };
    if p.is_empty() {
        return format!("2.4G {kind_cn}");
    }
    let pl = p.to_lowercase();
    let already = match kind {
        "Keyboard" => pl.contains("keyboard") || p.contains("键盘") || p.contains("X87"),
        "Mouse" => pl.contains("mouse") || p.contains("鼠标"),
        _ => true,
    };
    if already { p } else { format!("{p} {kind_cn}") }
}

/// 2.4G dongles often expose both keyboard and mouse collections.
/// Show the real device: ATK receiver → mouse, Sonix/EWEADN → keyboard.
pub fn emit_kinds(vid: u16, members: &[(u16, u16)]) -> Vec<&'static str> {
    let kinds = receiver_kinds(members);
    match vid {
        0x373B if kinds.iter().any(|k| *k == "Mouse") => vec!["Mouse"],
        0x0C45 if kinds.iter().any(|k| *k == "Keyboard") => vec!["Keyboard"],
        _ => kinds,
    }
}

pub fn compx_checksum(prefix: &[u8]) -> u8 {
    let sum = prefix.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    0x55u8.wrapping_sub(sum)
}

pub fn build_compx_cmd(report_id: u8, total_len: usize) -> Vec<u8> {
    let n = total_len.max(8);
    let mut report = vec![0u8; n];
    report[0] = report_id;
    report[1] = COMPX_CMD_BATTERY;
    let last = n - 1;
    report[last] = compx_checksum(&report[..last]);
    report
}

pub fn parse_compx_battery(report: &[u8]) -> Option<(u8, bool)> {
    // Live ATK 8K dongle frame (checksum-verified):
    //   08 04 00 00 00 02 PCT CHG VHI VLO 00.. CS
    // byte[6] is the percentage the peripheral itself reports; bytes[8..10]
    // are the cell voltage in mV. Prefer the reported percent — the mV→%
    // table saturates at 4110 mV, which turned a real 95% into "100%".
    if report.len() < 8 {
        return None;
    }
    let id = report[0];
    if id != COMPX_REPORT_ID && id != COMPX_REPORT_ID_ALT {
        return None;
    }
    if report[1] != COMPX_CMD_BATTERY {
        return None;
    }
    let charging = report[7] != 0;
    let raw = report[6];
    if (1..=100).contains(&raw) {
        return Some((raw, charging));
    }
    let voltage = if report.len() >= 10 {
        u16::from_be_bytes([report[8], report[9]])
    } else {
        0
    };
    if voltage == 0 {
        return None;
    }
    Some((voltage_to_percent(voltage, charging), charging))
}

/// Compx/VGN firmware voltage table (mV) → 0..=100.
pub fn voltage_to_percent(voltage_mv: u16, charging: bool) -> u8 {
    const TABLE: [u16; 21] = [
        3050, 3420, 3480, 3540, 3600, 3660, 3720, 3760, 3800, 3840, 3880, 3920, 3940, 3960, 3980,
        4000, 4020, 4040, 4060, 4080, 4110,
    ];
    if voltage_mv >= TABLE[TABLE.len() - 1] {
        return if charging { 99 } else { 100 };
    }
    let Some(idx) = TABLE.iter().position(|&v| voltage_mv < v) else {
        return if charging { 99 } else { 100 };
    };
    if idx == 0 {
        return 0;
    }
    let prev = TABLE[idx - 1] as f32;
    let step = (TABLE[idx] as f32 - prev) / 5.0;
    let mut level = ((voltage_mv as f32 - prev) / step + ((idx - 1) * 5) as f32).round() as i32;
    if level == 0 || level == 15 {
        level += 1;
    }
    level.clamp(0, 100) as u8
}

/// 前行者 X87 / Sonix 65-byte vendor status。只有 `0F 01 FE PCT` 子类型携带
/// 实时电量；周期性 `0F 01 01 <const>` 心跳里的字节是 dongle 冻结的陈旧缓存
/// （实测重启 + 充电都不变），绝不能当电量用——实时电量走 0x20 查询。
pub fn parse_sonix_battery(report: &[u8]) -> Option<u8> {
    fn pct_at(buf: &[u8], i: usize) -> Option<u8> {
        let p = *buf.get(i)?;
        (p <= 100).then_some(p)
    }
    if report.len() >= 5
        && report[0] == 0
        && report[1] == 0x0F
        && report[2] == 0x01
        && report[3] == 0xFE
    {
        return pct_at(report, 4);
    }
    if report.len() >= 4 && report[0] == 0x0F && report[1] == 0x01 && report[2] == 0xFE {
        return pct_at(report, 3);
    }
    None
}

/// Host request that matches the unsolicited status header (`0F 01`), not a
/// bare `0F 00` which the 2.4G dongle answers from a stale USB cache.
pub fn sonix_status_request(output_len: u16) -> Vec<u8> {
    let mut poke = vec![0u8; (output_len as usize).max(3)];
    poke[1] = 0x0F;
    poke[2] = 0x01;
    poke
}

/// EWEADN/Sonix 三模键盘电池应答（官方驱动同款协议，FF60/MI_03 通道）:
/// `00 20 <echo> 00 PCT 00... CS`，CS = 前 31 字节累加和。
pub fn parse_eweadn_battery(report: &[u8]) -> Option<u8> {
    if report.len() < 6 || report[0] != 0x00 || report[1] != 0x20 {
        return None;
    }
    let pct = *report.get(4)?;
    if pct > 100 {
        return None;
    }
    // 校验和（存在时）：前 len-1 字节累加 mod 256
    let last = report.len() - 1;
    let sum = report[..last].iter().fold(0u8, |a, &b| a.wrapping_add(b));
    if sum == report[last] || report[last] == 0 {
        Some(pct)
    } else {
        None
    }
}

/// 主动查询：向 FF60 写 `00 20 01`，读回电量。这是官方 EWEADN 驱动使用的
/// 同一条命令通道；对无此协议的集合只会超时返回 None。
pub(crate) fn query_eweadn_battery(handle: HANDLE, input_len: u16, output_len: u16) -> Option<u8> {
    if output_len < 4 || input_len < 6 {
        return None;
    }
    unsafe {
        let dbg0 = std::env::var_os("BTB_DEBUG").is_some();
        let _ = HidD_FlushQueue(handle);
        let cmd = qmk_cmd(output_len as usize, 0x20, &[0x01, 0x00]);
        let w = overlapped_write(handle, &cmd);
        if dbg0 {
            eprintln!(
                "eweadn write {:?}: {:?}",
                &cmd[..5],
                w.as_ref().map(|_| "ok")
            );
        }
        w.ok()?;
        // RF 往返可能超过半秒：每次读都给足到截止时间的余量，超时才放弃。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        let dbg = std::env::var_os("BTB_DEBUG").is_some();
        while std::time::Instant::now() < deadline {
            let remain = deadline.saturating_duration_since(std::time::Instant::now());
            let ms = remain.as_millis().max(1) as u32;
            match overlapped_read(handle, input_len.max(6) as usize, ms) {
                Ok(buf) => {
                    if dbg {
                        eprintln!("eweadn rsp: {}", to_hex(&buf));
                    }
                    if let Some(pct) = parse_eweadn_battery(&buf) {
                        return Some(pct);
                    }
                }
                Err(e) => {
                    if dbg {
                        eprintln!("eweadn read err: {e}");
                    }
                    break;
                }
            }
        }
        None
    }
}

/// QMK/Keychron wireless raw HID: command `0xA4`, percent in the next byte.
pub fn parse_qmk_battery(report: &[u8]) -> Option<u8> {
    let b = if report.first() == Some(&0) && report.len() > 1 {
        &report[1..]
    } else {
        report
    };
    if b.first() != Some(&0xA4) {
        return None;
    }
    let pct = *b.get(1)?;
    (pct <= 100).then_some(pct)
}

fn collections_to_devices(raw: &[HidDevice]) -> Vec<Device> {
    let mut groups: HashMap<(u16, u16), Vec<&HidDevice>> = HashMap::new();
    for d in raw {
        groups.entry((d.vid, d.pid)).or_default().push(d);
    }
    let mut out = Vec::new();
    for ((vid, pid), members) in groups {
        let usages: Vec<(u16, u16)> = members.iter().map(|m| (m.usage_page, m.usage)).collect();
        let path = members.first().map(|m| m.path.as_str()).unwrap_or("");
        let product = members
            .iter()
            .map(|m| m.product.trim())
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .to_string();
        if !include_receiver(vid, pid, &product, path, &usages) {
            continue;
        }
        let battery = members.iter().find_map(|m| m.battery);
        let wired = pid == 0x8006;
        let link = wired || members.iter().any(|m| m.link_up);
        let kinds = emit_kinds(vid, &usages);
        for kind in kinds {
            out.push(Device {
                name: display_name_for(vid, pid, &product, kind),
                address: format!("hid:{vid:04X}:{pid:04X}:{kind}"),
                is_le: false,
                connected: if is_sonix_vid(vid) {
                    link
                } else {
                    battery.is_some()
                },
                battery,
                status: if is_sonix_vid(vid) {
                    if link { "ok" } else { "offline" }
                } else if battery.is_some() {
                    "ok"
                } else {
                    "offline"
                },
                kind,
            });
        }
    }
    let mut out = merge_x87_rows(out);
    out.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

/// USB cable (PID 8006) and 2.4G dongle (PID FEFE) are the same keyboard.
/// Always keep the dongle address so one settings checkbox covers both links.
fn merge_x87_rows(devs: Vec<Device>) -> Vec<Device> {
    let mut wired: Option<Device> = None;
    let mut dongle: Option<Device> = None;
    let mut rest = Vec::new();
    for d in devs {
        let a = d.address.to_ascii_uppercase();
        if a.contains("0C45:8006") {
            wired = Some(d);
        } else if a.contains("0C45:FEFE") {
            dongle = Some(d);
        } else {
            rest.push(d);
        }
    }
    const CANON: &str = "hid:0C45:FEFE:Keyboard";
    match (wired, dongle) {
        (Some(w), Some(mut rf)) => {
            rf.name = "前行者 X87 有线".into();
            rf.address = CANON.into();
            rf.connected = w.connected || rf.connected;
            rf.status = if rf.connected { "ok" } else { "offline" };
            if rf.battery.is_none() {
                rf.battery = w.battery;
            }
            rest.push(rf);
        }
        (Some(mut w), None) => {
            w.name = "前行者 X87 有线".into();
            w.address = CANON.into();
            w.connected = true;
            w.status = "ok";
            rest.push(w);
        }
        (None, Some(mut rf)) => {
            rf.name = "前行者 X87 2.4G".into();
            rest.push(rf);
        }
        (None, None) => {}
    }
    rest
}

fn read_detail_path(detail: &[u8]) -> String {
    const OFFSET: usize = size_of::<u32>();
    if detail.len() <= OFFSET + 2 {
        return String::new();
    }
    let mut w: Vec<u16> = Vec::new();
    let n = (detail.len() - OFFSET) / 2;
    let base = unsafe { detail.as_ptr().add(OFFSET) } as *const u16;
    for k in 0..n {
        let c = unsafe { *base.add(k) };
        if c == 0 {
            break;
        }
        w.push(c);
    }
    String::from_utf16_lossy(&w)
}

pub(crate) fn probe_one(path: &str, query: bool) -> Result<Option<HidDevice>, String> {
    unsafe {
        let (handle, access) = open_hid(path).ok_or_else(|| "open failed".to_string())?;
        let _ = HidD_SetNumInputBuffers(handle, 64);

        let product = hid_product_string(handle);
        let manufacturer = hid_manufacturer_string(handle);

        let mut attr = zeroed::<HIDD_ATTRIBUTES>();
        attr.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        let _ = HidD_GetAttributes(handle, &mut attr);

        let mut pp = PHIDP_PREPARSED_DATA::default();
        if !hid_boolean_ok(HidD_GetPreparsedData(handle, &mut pp)) {
            let _ = CloseHandle(handle);
            return Ok(None);
        }
        let mut caps = zeroed::<HIDP_CAPS>();
        let _ = HidP_GetCaps(pp, &mut caps);

        let mut feature_hex = String::new();
        let mut debug = String::new();
        let mut battery = if query {
            read_standard_battery(handle, pp, &caps, &mut feature_hex)
        } else {
            None
        };
        // USB cable present = the keyboard is on this host, even with no wireless %.
        let mut link_up = attr.ProductID == 0x8006;

        // EWEADN/Sonix 三模键盘（前行者 X87 等）：官方驱动走 FF60/MI_03 的
        // 0x20 命令查电量，dongle 心跳里的字节是陈旧缓存不可信。
        if query
            && battery.is_none()
            && access == "R/W"
            && attr.VendorID == 0x0C45
            && caps.UsagePage != 0xFF59
        {
            if let Some(pct) = query_eweadn_battery(
                handle,
                caps.InputReportByteLength,
                caps.OutputReportByteLength,
            ) {
                battery = Some(pct);
                note_live_battery(attr.VendorID, attr.ProductID, pct);
                link_up = true;
            }
        }

        // Logitech HID++ (Unifying/Lightspeed receivers, USB devices): the
        // battery lives behind feature queries, not a fixed vendor report.
        if query && battery.is_none() && access == "R/W" && attr.VendorID == 0x046D {
            if let Some(pct) = crate::hidpp::query_battery(
                handle,
                caps.InputReportByteLength,
                caps.OutputReportByteLength,
                attr.VendorID,
                attr.ProductID,
            ) {
                battery = Some(pct);
                if feature_hex.is_empty() {
                    feature_hex = "hid++".into();
                }
                link_up = true;
            }
        }

        if query && battery.is_none() && access == "R/W" && is_vendor_page(caps.UsagePage) {
            let in_len = caps.InputReportByteLength;
            let out_len = caps.OutputReportByteLength;
            if is_compx_vid(attr.VendorID)
                && (in_len == 17 || in_len == 20 || out_len == 17 || out_len == 20)
            {
                match query_compx_battery(handle, in_len, out_len) {
                    Ok((pct, hex)) => {
                        battery = Some(pct);
                        note_live_battery(attr.VendorID, attr.ProductID, pct);
                        if feature_hex.is_empty() {
                            feature_hex = hex;
                        }
                    }
                    Err(e) => debug = e,
                }
            } else if is_sonix_vid(attr.VendorID)
                && attr.ProductID == 0xFEFE
                && (in_len == 65 || out_len == 65)
            {
                match query_sonix_status(handle, in_len, out_len) {
                    Ok((pct, hex, live)) => {
                        battery = Some(pct);
                        note_live_battery(attr.VendorID, attr.ProductID, pct);
                        link_up |= sonix_rf_alive(attr.VendorID, attr.ProductID, live);
                        if feature_hex.is_empty() {
                            feature_hex = hex;
                        }
                    }
                    Err(e) => {
                        debug = e;
                        link_up |= sonix_rf_alive(attr.VendorID, attr.ProductID, false);
                    }
                }
            } else if is_sonix_vid(attr.VendorID)
                && attr.ProductID == 0xFEFE
                && (in_len == 33 || out_len == 33)
            {
                if query_via_alive(handle, in_len, out_len) {
                    link_up |= sonix_rf_alive(attr.VendorID, attr.ProductID, true);
                }
            }
        }

        let _ = HidD_FreePreparsedData(pp);
        let _ = CloseHandle(handle);

        Ok(Some(HidDevice {
            path: path.to_string(),
            vid: attr.VendorID,
            pid: attr.ProductID,
            usage_page: caps.UsagePage,
            usage: caps.Usage,
            input_len: caps.InputReportByteLength,
            output_len: caps.OutputReportByteLength,
            feature_len: caps.FeatureReportByteLength,
            product,
            manufacturer,
            access,
            battery,
            feature_hex,
            debug,
            link_up,
        }))
    }
}

pub(crate) unsafe fn open_hid(path: &str) -> Option<(HANDLE, &'static str)> {
    let wpath = wide_path(path);
    let share = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0);
    let flags = FILE_FLAG_OVERLAPPED;
    let modes: [(u32, &str); 2] = [
        ((GENERIC_READ.0 | GENERIC_WRITE.0) as u32, "R/W"),
        (0u32, "ZERO"),
    ];
    for (access, tag) in modes {
        if let Ok(h) = CreateFileW(
            PCWSTR(wpath.as_ptr()),
            access,
            share,
            None,
            OPEN_EXISTING,
            flags,
            HANDLE::default(),
        ) {
            return Some((h, tag));
        }
    }
    // last resort: non-overlapped zero access (some stacks reject OVERLAPPED)
    let plain = FILE_FLAGS_AND_ATTRIBUTES(0);
    if let Ok(h) = CreateFileW(
        PCWSTR(wpath.as_ptr()),
        0,
        share,
        None,
        OPEN_EXISTING,
        plain,
        HANDLE::default(),
    ) {
        return Some((h, "ZERO"));
    }
    None
}

unsafe fn hid_wide_string(ok: BOOLEAN, prow: &[u16]) -> String {
    if !hid_boolean_ok(ok) {
        return String::new();
    }
    let end = prow.iter().position(|&c| c == 0).unwrap_or(prow.len());
    String::from_utf16_lossy(&prow[..end]).trim().to_string()
}

unsafe fn hid_product_string(handle: HANDLE) -> String {
    let mut prow = vec![0u16; 128];
    let ok = HidD_GetProductString(handle, prow.as_mut_ptr() as *mut _, (128 * 2) as u32);
    hid_wide_string(ok, &prow)
}

unsafe fn hid_manufacturer_string(handle: HANDLE) -> String {
    let mut prow = vec![0u16; 128];
    let ok = HidD_GetManufacturerString(handle, prow.as_mut_ptr() as *mut _, (128 * 2) as u32);
    hid_wide_string(ok, &prow)
}

unsafe fn read_standard_battery(
    handle: HANDLE,
    pp: PHIDP_PREPARSED_DATA,
    caps: &HIDP_CAPS,
    feature_hex: &mut String,
) -> Option<u8> {
    if caps.FeatureReportByteLength == 0 {
        return None;
    }
    let mut ids: HashSet<u8> = HashSet::new();
    ids.insert(0);
    ids.insert(6);
    collect_feature_report_ids(pp, caps, &mut ids);
    for id in ids {
        let len = caps.FeatureReportByteLength as usize;
        let mut report = vec![0u8; len];
        report[0] = id;
        if !hid_boolean_ok(HidD_GetFeature(
            handle,
            report.as_mut_ptr() as *mut _,
            len as u32,
        )) {
            continue;
        }
        if feature_hex.is_empty() {
            *feature_hex = to_hex(&report);
        }
        let found = read_usage(pp, HidP_Feature, 0x0006, 0x0020, &report)
            .or_else(|| read_usage(pp, HidP_Feature, 0x0084, 0x0045, &report))
            .or_else(|| read_usage(pp, HidP_Feature, 0x0084, 0x0040, &report))
            .or_else(|| read_usage(pp, HidP_Feature, 0x0084, 0x0066, &report));
        if found.is_some() {
            return found;
        }
    }
    None
}

unsafe fn collect_feature_report_ids(
    pp: PHIDP_PREPARSED_DATA,
    caps: &HIDP_CAPS,
    ids: &mut HashSet<u8>,
) {
    let mut n = caps.NumberFeatureValueCaps;
    if n == 0 {
        return;
    }
    let mut vcaps = vec![HIDP_VALUE_CAPS::default(); n as usize];
    let status = HidP_GetValueCaps(HidP_Feature, vcaps.as_mut_ptr(), &mut n, pp);
    if !hidp_success(status.0) {
        return;
    }
    for cap in vcaps.iter().take(n as usize) {
        ids.insert(cap.ReportID);
    }
}

pub(crate) unsafe fn query_compx_battery(
    handle: HANDLE,
    input_len: u16,
    output_len: u16,
) -> Result<(u8, String), String> {
    let attempts: [(u8, usize); 2] = [
        (COMPX_REPORT_ID, COMPX_PACKET),
        (COMPX_REPORT_ID_ALT, COMPX_PACKET_ALT),
    ];
    let mut last = String::from("no matching report size");
    for (rid, pkt) in attempts {
        let out_len = (output_len as usize).max(pkt);
        let in_len = (input_len as usize).max(pkt);
        if output_len != 0 && (output_len as usize) < pkt && (input_len as usize) < pkt {
            continue;
        }
        let mut cmd = build_compx_cmd(rid, pkt);
        if cmd.len() < out_len {
            cmd.resize(out_len, 0);
            let last_i = pkt.saturating_sub(1);
            if last_i < cmd.len() {
                cmd[last_i] = compx_checksum(&cmd[..last_i]);
            }
        }
        let _ = HidD_SetOutputReport(handle, cmd.as_ptr() as *const _, cmd.len() as u32);
        std::thread::sleep(Duration::from_millis(200));
        for try_n in 1..=COMPX_TRIES {
            match hid_write_then_read(handle, &cmd, in_len, TRANSACT_MS, true) {
                Ok(buf) => {
                    last = format!("rid={rid:02X} try={try_n} read:{}", to_hex(&buf));
                    if let Some((pct, _)) = parse_compx_battery(&buf) {
                        return Ok((pct, to_hex(&buf)));
                    }
                }
                Err(e) => last = format!("rid={rid:02X} try={try_n} {e}"),
            }
            if let Ok(buf) = hid_transact(handle, &cmd, in_len, TRANSACT_MS) {
                last = format!("rid={rid:02X} try={try_n} transact:{}", to_hex(&buf));
                if let Some((pct, _)) = parse_compx_battery(&buf) {
                    return Ok((pct, to_hex(&buf)));
                }
            }
        }
    }
    Err(last)
}

pub(crate) fn sonix_rf_alive(vid: u16, pid: u16, saw_packet: bool) -> bool {
    let Ok(mut map) = LIVE_LINKS.lock() else {
        return saw_packet;
    };
    let link = map.entry((vid, pid)).or_default();
    if saw_packet {
        link.touch_rf(Instant::now());
    }
    link.connected(Instant::now(), SONIX_LINK_TTL)
}

/// Returns (percent, hex, saw_rf_packet).
///
/// Do **not** FlushQueue first. The 前行者 dongle pushes `00 0F 01 ?? PCT`
/// every few tens of seconds; flushing then waiting 1s always misses it and
/// made 2.4G look offline. A queued 0x0F is consumed by this read — if the
/// keyboard is off, the next scan sees an empty queue.
unsafe fn query_sonix_status(
    handle: HANDLE,
    input_len: u16,
    output_len: u16,
) -> Result<(u8, String, bool), String> {
    let in_len = (input_len as usize).max(5);
    let _ = HidD_FlushQueue(handle);
    poke_sonix_status(handle, output_len);
    match overlapped_read(handle, in_len, 1500) {
        Ok(buf) => {
            if let Some(pct) = parse_sonix_battery(&buf) {
                return Ok((pct, to_hex(&buf), true));
            }
            Err(format!("unparsed {}", to_hex(&buf)))
        }
        Err(_) => Err("no sonix status (keyboard off?)".into()),
    }
}

pub(crate) unsafe fn query_via_alive(handle: HANDLE, input_len: u16, output_len: u16) -> bool {
    let cmd = qmk_cmd((output_len as usize).max(33), 0x01, &[]);
    match hid_write_then_read(handle, &cmd, (input_len as usize).max(2), 600, false) {
        Ok(buf) if looks_like_via_proto(&buf) => true,
        _ => false,
    }
}

pub(crate) unsafe fn query_via_battery(
    handle: HANDLE,
    input_len: u16,
    output_len: u16,
) -> Option<u8> {
    let in_len = (input_len as usize).max(2);
    let out_len = (output_len as usize).max(2);
    let cmds = [qmk_cmd(out_len, 0xA4, &[]), {
        let mut p = vec![0u8; out_len];
        p[0] = 0xA4;
        p
    }];
    for cmd in cmds {
        if let Ok(buf) = hid_write_then_read(handle, &cmd, in_len, 1200, false) {
            if let Some(pct) = parse_qmk_battery(&buf).or_else(|| parse_sonix_battery(&buf)) {
                return Some(pct);
            }
        }
    }
    None
}

pub(crate) unsafe fn poke_sonix_status(handle: HANDLE, output_len: u16) {
    let poke = sonix_status_request(output_len);
    let _ = overlapped_write(handle, &poke).is_ok()
        || hid_boolean_ok(HidD_SetOutputReport(
            handle,
            poke.as_ptr() as *const _,
            poke.len() as u32,
        ));
}

fn looks_like_via_proto(buf: &[u8]) -> bool {
    // VIA protocol version: report-id 0, command 0x01, then a non-zero version.
    let b = if buf.first() == Some(&0) && buf.len() > 1 {
        &buf[1..]
    } else {
        buf
    };
    if b.first() != Some(&0x01) || b.len() < 3 {
        return false;
    }
    // Ignore a leftover Sonix 0x0F that landed on the wrong collection.
    if b.get(1) == Some(&0x0F) {
        return false;
    }
    b[1] != 0 || b.get(2).copied().unwrap_or(0) != 0
}

#[allow(dead_code)]
unsafe fn drain_hid_input(handle: HANDLE, in_len: usize) {
    let _ = HidD_FlushQueue(handle);
    for _ in 0..12 {
        if overlapped_read(handle, in_len.max(1), 40).is_err() {
            break;
        }
    }
}

pub(crate) fn is_io_pending(err: &windows::core::Error) -> bool {
    // HRESULT_FROM_WIN32(ERROR_IO_PENDING=997) = 0x800703E5
    (err.code().0 as u32) == (0x80070000 | ERROR_IO_PENDING.0)
}

pub(crate) unsafe fn overlapped_write(handle: HANDLE, data: &[u8]) -> Result<(), String> {
    let event = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| e.to_string())?;
    let mut ov = OVERLAPPED::default();
    ov.hEvent = event;
    let result = WriteFile(handle, Some(data), None, Some(&mut ov as *mut _));
    let pending = match result {
        Ok(()) => false,
        Err(e) if is_io_pending(&e) => true,
        Err(e) => {
            let _ = CloseHandle(event);
            return Err(format!("WriteFile 0x{:08X}", e.code().0 as u32));
        }
    };
    if pending && !wait_overlapped(handle, &ov, TRANSACT_MS) {
        let _ = CloseHandle(event);
        return Err("write timeout".into());
    }
    let mut n = 0u32;
    let _ = GetOverlappedResult(handle, &ov, &mut n, false);
    let _ = CloseHandle(event);
    Ok(())
}

pub(crate) unsafe fn overlapped_read(
    handle: HANDLE,
    len: usize,
    timeout_ms: u32,
) -> Result<Vec<u8>, String> {
    let event = CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| e.to_string())?;
    let mut ov = OVERLAPPED::default();
    ov.hEvent = event;
    let mut buf = vec![0u8; len.max(1)];
    let result = ReadFile(handle, Some(&mut buf), None, Some(&mut ov as *mut _));
    let ok = match result {
        Ok(()) => true,
        Err(e) if is_io_pending(&e) => wait_overlapped(handle, &ov, timeout_ms),
        Err(e) => {
            let _ = CloseHandle(event);
            return Err(format!("ReadFile 0x{:08X}", e.code().0 as u32));
        }
    };
    let mut transferred = 0u32;
    if ok {
        let _ = GetOverlappedResult(handle, &ov, &mut transferred, false);
    }
    let _ = CloseHandle(event);
    if !ok {
        return Err("read timeout".into());
    }
    buf.truncate(transferred as usize);
    Ok(buf)
}

unsafe fn wait_overlapped(handle: HANDLE, ov: &OVERLAPPED, timeout_ms: u32) -> bool {
    let w = WaitForSingleObject(ov.hEvent, timeout_ms);
    if w == WAIT_OBJECT_0 {
        true
    } else {
        if w == WAIT_TIMEOUT {
            let _ = CancelIoEx(handle, Some(ov as *const _));
            let mut n = 0u32;
            let _ = GetOverlappedResult(handle, ov, &mut n, true);
        }
        false
    }
}

fn read_usage(
    pp: PHIDP_PREPARSED_DATA,
    rt: HIDP_REPORT_TYPE,
    page: u16,
    usage: u16,
    report: &[u8],
) -> Option<u8> {
    unsafe {
        let mut val: u32 = 0;
        let status = HidP_GetUsageValue(rt, page, 0, usage, &mut val, pp, report);
        if hidp_success(status.0) && val <= 100 {
            Some(val as u8)
        } else {
            None
        }
    }
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn wide_path(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_eweadn_battery_accepts_live_frame() {
        let mut r = vec![0u8; 33];
        r[0] = 0x00;
        r[1] = 0x20;
        r[2] = 0x01;
        r[3] = 0x00;
        r[4] = 0x64;
        let sum: u8 = r[..32].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        r[32] = sum;
        assert_eq!(parse_eweadn_battery(&r), Some(100));
        r[4] = 23;
        let sum2: u8 = r[..32].iter().fold(0u8, |a, &b| a.wrapping_add(b));
        r[32] = sum2;
        assert_eq!(parse_eweadn_battery(&r), Some(23));
    }

    #[test]
    fn parse_eweadn_battery_rejects_other_commands_and_bad_checksum() {
        let mut r = vec![0u8; 33];
        r[1] = 0x0F; // dongle heartbeat, not a battery answer
        r[4] = 17;
        assert_eq!(parse_eweadn_battery(&r), None);
        let mut b = vec![0u8; 33];
        b[1] = 0x20;
        b[4] = 50;
        b[32] = 0xAA; // wrong checksum
        assert_eq!(parse_eweadn_battery(&b), None);
        let mut c = vec![0u8; 33];
        c[1] = 0x20;
        c[4] = 101; // out of range
        c[32] = c[..32].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        assert_eq!(parse_eweadn_battery(&c), None);
    }

    use windows::Win32::Foundation::BOOLEAN;

    #[test]
    fn boolean_ok_matches_win32_true_false() {
        assert!(hid_boolean_ok(BOOLEAN(1)));
        assert!(hid_boolean_ok(BOOLEAN(0xFF)));
        assert!(!hid_boolean_ok(BOOLEAN(0)));
    }

    #[test]
    fn boolean_old_operator_was_always_true() {
        // The previous check `!b.0 != 0` is bitwise-NOT, so both 0 and 1 pass.
        let wrong = |v: u8| (!v) != 0;
        assert!(wrong(0));
        assert!(wrong(1));
        assert!(!hid_boolean_ok(BOOLEAN(0)));
    }

    #[test]
    fn hidp_success_is_not_zero() {
        assert!(!hidp_success(0));
        assert!(hidp_success(HIDP_STATUS_SUCCESS.0));
    }

    #[test]
    fn compx_checksum_and_packet() {
        let cmd = build_compx_cmd(0x08, 17);
        assert_eq!(cmd.len(), 17);
        assert_eq!(cmd[0], 0x08);
        assert_eq!(cmd[1], 0x04);
        assert_eq!(cmd[16], compx_checksum(&cmd[..16]));
        assert_eq!(cmd[16], 0x55u8.wrapping_sub(0x08u8.wrapping_add(0x04)));
    }

    #[test]
    fn parse_compx_battery_ok() {
        let mut r = vec![0u8; 17];
        r[0] = 0x08;
        r[1] = 0x04;
        r[6] = 67;
        r[7] = 0;
        assert_eq!(parse_compx_battery(&r), Some((67, false)));
        r[7] = 1;
        assert_eq!(parse_compx_battery(&r), Some((67, true)));
    }

    #[test]
    fn parse_compx_battery_rejects_garbage() {
        assert_eq!(parse_compx_battery(&[0x08, 0x99, 0, 0, 0, 0, 10, 0]), None);
        assert_eq!(parse_compx_battery(&[0x01, 0x04, 0, 0, 0, 0, 10, 0]), None);
        let mut r = vec![0u8; 17];
        r[0] = 0x08;
        r[1] = 0x04;
        r[6] = 200;
        assert_eq!(parse_compx_battery(&r), None);
    }

    #[test]
    fn include_compx_even_without_vendor_page_listed() {
        let members = [(0x0001, 0x0002), (0x0001, 0x0006)];
        assert!(include_receiver(
            0x373B,
            0x101B,
            "ATK Mouse 8K Dongle",
            r"\\?\hid#vid_373b",
            &members
        ));
    }

    #[test]
    fn skip_virtual_and_i2c_noise() {
        let members = [(0x0001, 0x0006), (0x0001, 0x0002), (0xFF00, 0x0001)];
        assert!(!include_receiver(
            0x00FF,
            0x0001,
            "Virtual Multitouch Device",
            r"\\?\hid#gvinput",
            &members
        ));
        assert!(!include_receiver(
            0x2808,
            0x0001,
            "HIDI2C Device",
            r"\\?\hid#asuf1204",
            &members
        ));
    }

    #[test]
    fn include_named_24g_dongle() {
        let members = [(0x0001, 0x0002), (0x0001, 0x0006), (0xFF59, 0x0061)];
        assert!(include_receiver(
            0x0C45,
            0xFEFE,
            "2.4G Dongle",
            r"\\?\hid#vid_0c45&pid_fefe",
            &members
        ));
        assert!(!include_receiver(
            0x0C45,
            0xFEFE,
            "USB Camera",
            r"\\?\hid#vid_0c45&pid_fefe",
            &members
        ));
    }

    #[test]
    fn generic_receiver_needs_vendor_plus_mouse_or_keyboard() {
        assert!(include_receiver(
            0x1234,
            0x0001,
            "Widget",
            r"\\?\hid#vid_1234",
            &[(0x0001, 0x0002), (0xFF02, 0x0002)]
        ));
        assert!(!include_receiver(
            0x1234,
            0x0001,
            "Widget",
            r"\\?\hid#vid_1234",
            &[(0x0001, 0x0002)]
        ));
        assert!(!include_receiver(
            0x1234,
            0x0001,
            "Widget",
            r"\\?\hid#vid_1234",
            &[(0xFF02, 0x0002)]
        ));
    }

    #[test]
    fn kinds_from_usages() {
        assert_eq!(
            receiver_kinds(&[(0x0001, 0x0002), (0x0001, 0x0006), (0xFF02, 0x0002)]),
            vec!["Mouse", "Keyboard"]
        );
        assert_eq!(receiver_kinds(&[(0xFF02, 0x0002)]), vec!["Unknown"]);
    }

    #[test]
    fn display_name_appends_kind() {
        assert_eq!(display_name("2.4G Receiver", "Mouse"), "2.4G Receiver 鼠标");
        assert_eq!(display_name("", "Keyboard"), "2.4G 键盘");
        assert_eq!(display_name("HID Keyboard", "Keyboard"), "HID Keyboard");
        assert_eq!(
            display_name("ATK Mouse 8K Dongle", "Keyboard"),
            "ATK Mouse 8K Dongle 键盘"
        );
        assert_eq!(
            display_name("ATK Mouse 8K Dongle", "Mouse"),
            "ATK Mouse 8K Dongle"
        );
    }

    #[test]
    fn parse_compx_live_atk_8k_packet() {
        let r = [
            0x08, 0x04, 0x00, 0x00, 0x00, 0x02, 0x32, 0x00, 0x0F, 0x2B, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xDB,
        ];
        assert_eq!(parse_compx_battery(&r), Some((50, false)));
    }

    #[test]
    fn parse_compx_prefers_reported_percent_over_voltage_table() {
        // Real frame from an ATK 8K dongle at ~95% charge. The mV→% table
        // saturates at 4110 mV so the voltage path would say 100; byte[6]=0x5F
        // (95) is what the device itself reports and must win.
        let r = [
            0x08, 0x04, 0x00, 0x00, 0x00, 0x02, 0x5F, 0x00, 0x10, 0x1A, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xBE,
        ];
        assert_eq!(parse_compx_battery(&r), Some((95, false)));
    }

    #[test]
    fn parse_compx_falls_back_to_voltage_when_percent_missing() {
        let mut r = vec![0u8; 17];
        r[0] = 0x08;
        r[1] = 0x04;
        r[6] = 0;
        r[7] = 1;
        r[8] = 0x0F;
        r[9] = 0x2B; // 3883 mV -> 50%
        assert_eq!(parse_compx_battery(&r), Some((50, true)));
    }

    #[test]
    fn voltage_to_percent_table() {
        assert_eq!(voltage_to_percent(3049, false), 0);
        assert_eq!(voltage_to_percent(4110, false), 100);
        assert_eq!(voltage_to_percent(4110, true), 99);
        assert_eq!(voltage_to_percent(3883, false), 50);
    }

    #[test]
    fn parse_sonix_x87_status() {
        // The periodic heartbeat (`01` subtype) carries a frozen cache value:
        // never treat it as battery.
        let mut r = vec![0u8; 65];
        r[1] = 0x0F;
        r[2] = 0x01;
        r[3] = 0x01;
        r[4] = 0x11;
        assert_eq!(parse_sonix_battery(&r), None);
        r[1] = 0x99;
        r[4] = 50;
        assert_eq!(parse_sonix_battery(&r), None);
        // The `FE` subtype is a live battery event.
        let live = {
            let mut p = vec![0u8; 65];
            p[1] = 0x0F;
            p[2] = 0x01;
            p[3] = 0xFE;
            p[4] = 0x0E;
            p
        };
        assert_eq!(parse_sonix_battery(&live), Some(14));
        let rid_f = vec![0x0F, 0x01, 0xFE, 42, 0];
        assert_eq!(parse_sonix_battery(&rid_f), Some(42));
    }

    #[test]
    fn sonix_status_request_asks_for_0f01() {
        let r = sonix_status_request(65);
        assert_eq!(r.len(), 65);
        assert_eq!(&r[..3], &[0x00, 0x0F, 0x01]);
        assert!(r[3..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_qmk_wireless_battery() {
        assert_eq!(parse_qmk_battery(&[0x00, 0xA4, 63]), Some(63));
        assert_eq!(parse_qmk_battery(&[0xA4, 8, 0]), Some(8));
        assert_eq!(parse_qmk_battery(&[0x00, 0xA4, 101]), None);
        assert_eq!(parse_qmk_battery(&[0x00, 0x01, 0x09]), None);
    }

    #[test]
    fn emit_kinds_picks_real_device() {
        let both = [(0x0001, 0x0002), (0x0001, 0x0006), (0xFF02, 0x0002)];
        assert_eq!(emit_kinds(0x373B, &both), vec!["Mouse"]);
        assert_eq!(emit_kinds(0x0C45, &both), vec!["Keyboard"]);
        assert_eq!(emit_kinds(0x1234, &both), vec!["Mouse", "Keyboard"]);
    }

    #[test]
    fn display_name_eweadn_x87() {
        assert_eq!(
            display_name_for(0x0C45, 0xFEFE, "2.4G Dongle", "Keyboard"),
            "前行者 X87 2.4G"
        );
        assert_eq!(
            display_name_for(0x0C45, 0x8006, "X87", "Keyboard"),
            "前行者 X87 有线"
        );
        assert_eq!(
            parse_hid_addr("hid:373B:101B:Mouse"),
            Some((0x373B, 0x101B))
        );
    }

    #[test]
    fn vid_pid_from_hid_path_parses_x87_dongle() {
        let p = r"\\?\hid#vid_0c45&pid_fefe&mi_04#9&32b46102&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(vid_pid_from_hid_path(p), Some((0x0C45, 0xFEFE)));
        assert!(hid_path_is_x87_24g(p));
        assert!(!hid_path_is_x87_24g(
            r"\\?\hid#vid_0c45&pid_8006&mi_00#9&1dc44599&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}"
        ));
        assert_eq!(
            vid_pid_from_hid_path(r"\\?\hid#vid_373b&pid_101b&mi_01&col05#9&f987dd2&0&0004"),
            Some((0x373B, 0x101B))
        );
    }

    #[test]
    fn key_activity_marks_24g_connected_without_battery() {
        LIVE_LINKS.lock().unwrap().clear();
        assert!(note_x87_key_activity());
        assert!(sonix_rf_alive(0x0C45, 0xFEFE, false));
        assert!(!note_x87_key_activity());
        LIVE_LINKS.lock().unwrap().clear();
    }

    #[test]
    fn live_link_packet_connects_until_ttl() {
        let mut link = LiveLink::default();
        let t0 = Instant::now();
        link.on_battery(17, t0);
        assert_eq!(link.battery, Some(17));
        assert!(link.connected(t0, Duration::from_secs(90)));
        assert!(link.connected(t0 + Duration::from_secs(89), Duration::from_secs(90)));
        assert!(!link.connected(t0 + Duration::from_secs(91), Duration::from_secs(90)));
    }

    #[test]
    fn live_link_optimistic_touch_connects_without_battery() {
        let mut link = LiveLink::default();
        let t0 = Instant::now();
        link.touch_rf(t0);
        assert!(link.connected(t0, Duration::from_secs(90)));
        assert_eq!(link.battery, None);
    }

    #[test]
    fn live_link_wired_is_connected_without_rf() {
        let mut link = LiveLink::default();
        link.set_wired(true);
        assert!(link.connected(Instant::now(), Duration::from_secs(1)));
        link.set_wired(false);
        assert!(!link.connected(Instant::now(), Duration::from_secs(1)));
    }

    #[test]
    fn overlay_live_applies_rf_battery_and_link() {
        LIVE_LINKS.lock().unwrap().clear();
        {
            let mut map = LIVE_LINKS.lock().unwrap();
            map.entry((0x0C45, 0xFEFE))
                .or_default()
                .on_battery(22, Instant::now());
        }
        let mut raw = [
            dummy_hid(0x0C45, 0xFEFE, 0x0001, 0x0006, "2.4G Dongle", None),
            dummy_hid(0x0C45, 0xFEFE, 0xFF59, 0x0061, "2.4G Dongle", None),
        ];
        overlay_live(&mut raw);
        assert_eq!(raw[1].battery, Some(22));
        assert!(raw.iter().any(|d| d.link_up));
        let devs = collections_to_devices(&raw);
        assert_eq!(devs.len(), 1);
        assert!(devs[0].connected);
        assert_eq!(devs[0].battery, Some(22));
        assert_eq!(devs[0].name, "前行者 X87 2.4G");
        LIVE_LINKS.lock().unwrap().clear();
    }

    fn dummy_hid(
        vid: u16,
        pid: u16,
        page: u16,
        usage: u16,
        product: &str,
        battery: Option<u8>,
    ) -> HidDevice {
        dummy_hid_ex(vid, pid, page, usage, product, battery, false)
    }

    fn dummy_hid_ex(
        vid: u16,
        pid: u16,
        page: u16,
        usage: u16,
        product: &str,
        battery: Option<u8>,
        link_up: bool,
    ) -> HidDevice {
        HidDevice {
            path: format!("\\\\?\\hid#vid_{vid:04x}&pid_{pid:04x}"),
            vid,
            pid,
            usage_page: page,
            usage,
            input_len: 0,
            output_len: 0,
            feature_len: 0,
            product: product.into(),
            manufacturer: String::new(),
            access: "R/W",
            battery,
            feature_hex: String::new(),
            debug: String::new(),
            link_up,
        }
    }

    #[test]
    fn group_atk_mouse_and_x87_keyboard() {
        let raw = [
            dummy_hid(0x373B, 0x101B, 0x0001, 0x0002, "ATK Mouse 8K Dongle", None),
            dummy_hid(0x373B, 0x101B, 0x0001, 0x0006, "ATK Mouse 8K Dongle", None),
            dummy_hid(
                0x373B,
                0x101B,
                0xFF02,
                0x0002,
                "ATK Mouse 8K Dongle",
                Some(50),
            ),
            dummy_hid(0x0C45, 0xFEFE, 0x0001, 0x0006, "2.4G Dongle", None),
            dummy_hid(0x0C45, 0xFEFE, 0x0001, 0x0002, "2.4G Dongle", None),
            dummy_hid_ex(
                0x0C45,
                0xFEFE,
                0xFF59,
                0x0061,
                "2.4G Dongle",
                Some(17),
                true,
            ),
        ];
        let mut devs = collections_to_devices(&raw);
        devs.sort_by(|a, b| a.address.cmp(&b.address));
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].name, "前行者 X87 2.4G");
        assert_eq!(devs[0].kind, "Keyboard");
        assert_eq!(devs[0].battery, Some(17));
        assert!(devs[0].connected);
        assert_eq!(devs[1].name, "ATK Mouse 8K Dongle");
        assert_eq!(devs[1].kind, "Mouse");
        assert_eq!(devs[1].battery, Some(50));
    }

    #[test]
    fn skip_unrelated_sonix_keeps_x87_dongle() {
        let members_kb = [(0x0001, 0x0006), (0xFF00, 0x0001)];
        assert!(!include_receiver(
            0x0C45,
            0x8001,
            "EWEADN X87",
            r"\\?\hid#vid_0c45&pid_8001",
            &members_kb
        ));
        assert!(include_receiver(
            0x0C45,
            0xFEFE,
            "USB Composite Device",
            r"\\?\hid#vid_0c45&pid_fefe",
            &members_kb
        ));
        assert!(include_receiver(
            0x0C45,
            0x8006,
            "X87",
            r"\\?\hid#vid_0c45&pid_8006",
            &members_kb
        ));
        let raw = [
            dummy_hid_ex(0x0C45, 0xFEFE, 0x0001, 0x0006, "2.4G Dongle", None, true),
            dummy_hid_ex(
                0x0C45,
                0xFEFE,
                0xFF59,
                0x0061,
                "2.4G Dongle",
                Some(17),
                true,
            ),
            dummy_hid(0x0C45, 0x8001, 0x0001, 0x0006, "EWEADN X87", None),
            dummy_hid(0x0C45, 0x8001, 0xFF00, 0x0001, "EWEADN X87", None),
        ];
        let devs = collections_to_devices(&raw);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].name, "前行者 X87 2.4G");
        assert_eq!(devs[0].address, "hid:0C45:FEFE:Keyboard");
        assert!(devs[0].connected);
    }

    #[test]
    fn leftover_sonix_battery_is_not_connected() {
        let raw = [
            dummy_hid(0x0C45, 0xFEFE, 0x0001, 0x0006, "2.4G Dongle", None),
            dummy_hid(0x0C45, 0xFEFE, 0xFF59, 0x0061, "2.4G Dongle", Some(17)),
        ];
        let devs = collections_to_devices(&raw);
        assert_eq!(devs.len(), 1);
        assert!(!devs[0].connected);
        assert_eq!(devs[0].battery, Some(17));
        assert_eq!(devs[0].status, "offline");
    }

    #[test]
    fn rf_status_packet_marks_24g_connected() {
        let raw = [
            dummy_hid(0x0C45, 0xFEFE, 0x0001, 0x0006, "2.4G Dongle", None),
            dummy_hid_ex(
                0x0C45,
                0xFEFE,
                0xFF59,
                0x0061,
                "2.4G Dongle",
                Some(17),
                true,
            ),
        ];
        let devs = collections_to_devices(&raw);
        assert_eq!(devs.len(), 1);
        assert!(devs[0].connected);
        assert_eq!(devs[0].status, "ok");
        assert_eq!(devs[0].battery, Some(17));
    }

    #[test]
    fn sonix_rf_alive_remembers_last_packet() {
        LIVE_LINKS.lock().unwrap().clear();
        assert!(!sonix_rf_alive(0x0C45, 0xABCD, false));
        assert!(sonix_rf_alive(0x0C45, 0xABCD, true));
        assert!(sonix_rf_alive(0x0C45, 0xABCD, false));
        LIVE_LINKS.lock().unwrap().clear();
    }

    #[test]
    fn wired_x87_is_connected_and_merges_with_dongle() {
        let raw = [
            dummy_hid(0x0C45, 0xFEFE, 0x0001, 0x0006, "2.4G Dongle", None),
            dummy_hid(0x0C45, 0xFEFE, 0xFF59, 0x0061, "2.4G Dongle", Some(17)),
            dummy_hid(0x0C45, 0x8006, 0x0001, 0x0006, "X87", None),
            dummy_hid(0x0C45, 0x8006, 0xFF13, 0x0001, "X87", None),
        ];
        let devs = collections_to_devices(&raw);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].name, "前行者 X87 有线");
        assert_eq!(devs[0].address, "hid:0C45:FEFE:Keyboard");
        assert!(devs[0].connected);
        assert_eq!(devs[0].status, "ok");
        assert_eq!(devs[0].battery, Some(17));
    }

    #[test]
    fn wired_x87_alone_uses_dongle_address() {
        let raw = [
            dummy_hid(0x0C45, 0x8006, 0x0001, 0x0006, "X87", None),
            dummy_hid(0x0C45, 0x8006, 0xFF13, 0x0001, "X87", None),
        ];
        let devs = collections_to_devices(&raw);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].address, "hid:0C45:FEFE:Keyboard");
        assert_eq!(devs[0].name, "前行者 X87 有线");
        assert!(devs[0].connected);
        assert_eq!(devs[0].battery, None);
    }

    #[test]
    fn via_proto_response_is_recognized() {
        assert!(looks_like_via_proto(&[0x00, 0x01, 0x09, 0x00]));
        assert!(!looks_like_via_proto(&[0x00, 0x0F, 0x01, 0x01, 0x11]));
        assert!(!looks_like_via_proto(&[0x00, 0x01, 0x00, 0x00]));
        assert!(!looks_like_via_proto(&[]));
    }

    #[test]
    fn unplug_wired_x87_marks_dongle_connected() {
        HAD_X87_WIRED.store(false, Ordering::SeqCst);
        LIVE_LINKS.lock().unwrap().clear();
        let mut wired = vec![Device {
            name: "前行者 X87".into(),
            address: "hid:0C45:FEFE:Keyboard".into(),
            is_le: false,
            connected: true,
            battery: None,
            status: "ok",
            kind: "Keyboard",
        }];
        apply_x87_mode_transition(&mut wired, true);
        let mut rf = vec![Device {
            name: "前行者 X87".into(),
            address: "hid:0C45:FEFE:Keyboard".into(),
            is_le: false,
            connected: false,
            battery: None,
            status: "offline",
            kind: "Keyboard",
        }];
        apply_x87_mode_transition(&mut rf, false);
        assert!(rf[0].connected);
        assert_eq!(rf[0].status, "ok");
        LIVE_LINKS.lock().unwrap().clear();
        HAD_X87_WIRED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn dongle_without_live_battery_is_offline() {
        let raw = [
            dummy_hid(0x0C45, 0xFEFE, 0x0001, 0x0006, "2.4G Dongle", None),
            dummy_hid(0x0C45, 0xFEFE, 0xFF59, 0x0061, "2.4G Dongle", None),
        ];
        let devs = collections_to_devices(&raw);
        assert_eq!(devs.len(), 1);
        assert!(!devs[0].connected);
        assert_eq!(devs[0].battery, None);
        assert_eq!(devs[0].status, "offline");
    }

    #[test]
    fn stale_cache_does_not_mark_dongle_connected() {
        BATTERY_CACHE
            .lock()
            .unwrap()
            .insert((0x0C45, 0xFEFE), (17, Instant::now()));
        let mut devs = vec![Device {
            name: "前行者 X87".into(),
            address: "hid:0C45:FEFE:Keyboard".into(),
            is_le: false,
            connected: false,
            battery: None,
            status: "offline",
            kind: "Keyboard",
        }];
        apply_battery_cache(&mut devs);
        // 缓存不得翻转连接状态；离线设备也不显示历史电量。
        assert!(!devs[0].connected);
        assert_eq!(devs[0].battery, None);
        assert_eq!(devs[0].status, "offline");
    }

    #[test]
    fn cache_does_not_clear_wired_connection() {
        BATTERY_CACHE
            .lock()
            .unwrap()
            .insert((0x0C45, 0xFEFE), (17, Instant::now()));
        let mut devs = vec![Device {
            name: "前行者 X87".into(),
            address: "hid:0C45:FEFE:Keyboard".into(),
            is_le: false,
            connected: true,
            battery: None,
            status: "ok",
            kind: "Keyboard",
        }];
        apply_battery_cache(&mut devs);
        assert!(devs[0].connected);
        assert_eq!(devs[0].battery, Some(17));
        assert_eq!(devs[0].status, "ok");
    }
}
