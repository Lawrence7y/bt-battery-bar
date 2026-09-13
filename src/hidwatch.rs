//! Persistent HID listeners so 2.4G status/battery update as reports arrive
//! instead of only during the sparse poll loop.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Devices::HumanInterfaceDevice::{HidD_FlushQueue, HidD_SetNumInputBuffers};
use windows::Win32::Foundation::CloseHandle;

use crate::hid::{
    HidDevice, devices_from_raw, enumerate_ex, is_compx_vid, is_hidpp_vid, is_io_pending,
    is_sonix_vid, is_vendor_page, list_hid_paths, note_live_battery, open_hid, overlapped_read,
    parse_sonix_battery, poke_sonix_status, probe_one, query_compx_battery, query_via_alive,
    query_via_battery, sonix_rf_alive, to_hex,
};
use crate::model::Device;

const COMPX_PERIOD: Duration = Duration::from_secs(8);
const SUPERVISOR_SLICE: Duration = Duration::from_millis(100);
const SUPERVISOR_ITERS: u32 = 20;
const SONIX_WAIT_MS: u32 = 250;
static SONIX_PKT_LOGS: AtomicU32 = AtomicU32::new(0);
static VIA_LOGS: AtomicU32 = AtomicU32::new(0);

enum WatchKind {
    SonixStatus,
    SonixVia,
    CompxBattery,
    /// 罗技 HID++ 特征查询（0x1002/0x1000/0x1001）。
    HidppBattery,
}

pub struct Watcher {
    stop: Arc<AtomicBool>,
    kick: Arc<Mutex<bool>>,
    poke: Arc<AtomicBool>,
}

impl Watcher {
    pub fn start(on_update: Arc<dyn Fn(Vec<Device>) + Send + Sync>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let kick = Arc::new(Mutex::new(true));
        let poke = Arc::new(AtomicBool::new(true));
        let stop_t = stop.clone();
        let kick_t = kick.clone();
        let poke_t = poke.clone();
        let _ = thread::Builder::new()
            .name("hid-watch".into())
            .spawn(move || supervisor(stop_t, kick_t, poke_t, on_update));
        Self { stop, kick, poke }
    }

    pub fn kick(&self) {
        self.poke.store(true, Ordering::SeqCst);
        if let Ok(mut k) = self.kick.lock() {
            *k = true;
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.kick();
    }
}

fn watch_kind(d: &HidDevice) -> Option<WatchKind> {
    if d.access != "R/W" || !is_vendor_page(d.usage_page) {
        return None;
    }
    if is_sonix_vid(d.vid) && (d.pid == 0xFEFE || d.pid == 0x8006) && d.input_len == 65 {
        return Some(WatchKind::SonixStatus);
    }
    if is_sonix_vid(d.vid) && d.pid == 0xFEFE && d.input_len == 33 {
        return Some(WatchKind::SonixVia);
    }
    if is_compx_vid(d.vid)
        && (d.input_len == 17 || d.input_len == 20 || d.output_len == 17 || d.output_len == 20)
    {
        return Some(WatchKind::CompxBattery);
    }
    // 罗技 HID++（Unifying / Lightspeed / 有线）。此前只有 CLI 的
    // `enumerate(query=true)` 会查它，GUI 从不调用，于是 README/商店描述里的
    // “罗技支持”在用户实际运行的界面里永远是 `--`。
    if is_hidpp_vid(d.vid) {
        return Some(WatchKind::HidppBattery);
    }
    None
}

fn supervisor(
    stop: Arc<AtomicBool>,
    kick: Arc<Mutex<bool>>,
    poke: Arc<AtomicBool>,
    on_update: Arc<dyn Fn(Vec<Device>) + Send + Sync>,
) {
    let last_raw: Arc<Mutex<Vec<HidDevice>>> = Arc::new(Mutex::new(Vec::new()));
    let mut children: HashMap<String, Arc<AtomicBool>> = HashMap::new();
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let paths = list_hid_paths();
        let present: HashSet<String> = paths.iter().cloned().collect();
        let mut raw: Vec<HidDevice> = Vec::new();
        {
            let prev = last_raw.lock().map(|g| g.clone()).unwrap_or_default();
            for path in &paths {
                if let Some(old) = prev.iter().find(|d| d.path == *path) {
                    raw.push(old.clone());
                    continue;
                }
                match probe_one(path, false) {
                    Ok(Some(d)) => raw.push(d),
                    _ => {}
                }
            }
        }
        children.retain(|path, child_stop| {
            if present.contains(path) {
                true
            } else {
                child_stop.store(true, Ordering::SeqCst);
                false
            }
        });
        for d in &raw {
            if children.contains_key(&d.path) {
                continue;
            }
            let Some(kind) = watch_kind(d) else {
                continue;
            };
            let child_stop = Arc::new(AtomicBool::new(false));
            spawn_child(
                d.clone(),
                kind,
                stop.clone(),
                child_stop.clone(),
                poke.clone(),
                last_raw.clone(),
                on_update.clone(),
            );
            children.insert(d.path.clone(), child_stop);
        }
        let identity_changed = {
            last_raw
                .lock()
                .map(|prev| {
                    prev.len() != raw.len()
                        || prev.iter().any(|d| !present.contains(&d.path))
                        || raw.iter().any(|d| prev.iter().all(|p| p.path != d.path))
                })
                .unwrap_or(true)
        };
        let kicked_now = kick
            .lock()
            .map(|mut k| {
                let v = *k;
                *k = false;
                v
            })
            .unwrap_or(false);
        if let Ok(mut slot) = last_raw.lock() {
            *slot = raw.clone();
        }
        if kicked_now || identity_changed {
            on_update(devices_from_raw(raw, false));
        }

        for _ in 0..SUPERVISOR_ITERS {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let kicked = kick
                .lock()
                .map(|mut k| {
                    let v = *k;
                    *k = false;
                    v
                })
                .unwrap_or(false);
            if kicked {
                break;
            }
            thread::sleep(SUPERVISOR_SLICE);
        }
    }
    for child_stop in children.values() {
        child_stop.store(true, Ordering::SeqCst);
    }
}

fn publish(last_raw: &Mutex<Vec<HidDevice>>, on_update: &dyn Fn(Vec<Device>)) {
    let raw = last_raw.lock().map(|g| g.clone()).unwrap_or_default();
    on_update(devices_from_raw(raw, false));
}

fn spawn_child(
    d: HidDevice,
    kind: WatchKind,
    parent_stop: Arc<AtomicBool>,
    child_stop: Arc<AtomicBool>,
    poke: Arc<AtomicBool>,
    last_raw: Arc<Mutex<Vec<HidDevice>>>,
    on_update: Arc<dyn Fn(Vec<Device>) + Send + Sync>,
) {
    let name = match kind {
        WatchKind::SonixStatus => "hid-sonix",
        WatchKind::SonixVia => "hid-via",
        WatchKind::CompxBattery => "hid-compx",
        WatchKind::HidppBattery => "hid-hidpp",
    };
    let _ = thread::Builder::new().name(name.into()).spawn(move || {
        while !parent_stop.load(Ordering::SeqCst) && !child_stop.load(Ordering::SeqCst) {
            let opened = unsafe { open_hid(&d.path) };
            let Some((handle, tag)) = opened else {
                thread::sleep(Duration::from_millis(800));
                continue;
            };
            crate::dblog::log(&format!(
                "hid watch {name} {:04X}:{:04X} in{} acc={tag}",
                d.vid, d.pid, d.input_len
            ));
            unsafe {
                let _ = HidD_SetNumInputBuffers(handle, 64);
            }
            match kind {
                WatchKind::SonixStatus => watch_sonix(
                    handle,
                    &d,
                    &parent_stop,
                    &child_stop,
                    &poke,
                    &last_raw,
                    &*on_update,
                ),
                WatchKind::SonixVia => watch_via(
                    handle,
                    &d,
                    &parent_stop,
                    &child_stop,
                    &last_raw,
                    &*on_update,
                ),
                WatchKind::CompxBattery => watch_compx(
                    handle,
                    &d,
                    &parent_stop,
                    &child_stop,
                    &last_raw,
                    &*on_update,
                ),
                WatchKind::HidppBattery => watch_hidpp(
                    handle,
                    &d,
                    &parent_stop,
                    &child_stop,
                    &last_raw,
                    &*on_update,
                ),
            }
            unsafe {
                let _ = CloseHandle(handle);
            }
        }
    });
}

fn stopped(parent: &AtomicBool, child: &AtomicBool) -> bool {
    parent.load(Ordering::SeqCst) || child.load(Ordering::SeqCst)
}

fn watch_sonix(
    handle: windows::Win32::Foundation::HANDLE,
    d: &HidDevice,
    parent_stop: &AtomicBool,
    child_stop: &AtomicBool,
    poke: &AtomicBool,
    last_raw: &Mutex<Vec<HidDevice>>,
    on_update: &dyn Fn(Vec<Device>),
) {
    use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::Storage::FileSystem::ReadFile;
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::core::PCWSTR;

    let in_len = (d.input_len as usize).max(5);
    if d.pid == 0xFEFE {
        unsafe {
            let _ = HidD_FlushQueue(handle);
        }
    }
    let Ok(event) = (unsafe { CreateEventW(None, false, false, PCWSTR::null()) }) else {
        return;
    };
    let mut last_up = sonix_rf_alive(d.vid, d.pid, false);
    let mut last_poke = Instant::now() - Duration::from_secs(30);
    let mut last_pct: Option<u8> = None;
    while !stopped(parent_stop, child_stop) {
        let mut ov = OVERLAPPED::default();
        ov.hEvent = event;
        let mut buf = vec![0u8; in_len];
        let pending = unsafe {
            match ReadFile(
                handle,
                Some(buf.as_mut_slice()),
                None,
                Some(&mut ov as *mut _),
            ) {
                Ok(()) => false,
                Err(e) if is_io_pending(&e) => true,
                Err(_) => break,
            }
        };
        let got = if !pending {
            let mut n = 0u32;
            unsafe {
                let _ = GetOverlappedResult(handle, &ov, &mut n, false);
            }
            buf.truncate(n as usize);
            Some(buf)
        } else {
            let out;
            loop {
                if stopped(parent_stop, child_stop) {
                    unsafe {
                        let _ = CancelIoEx(handle, Some(&ov as *const _));
                        let mut n = 0u32;
                        let _ = GetOverlappedResult(handle, &ov, &mut n, true);
                    }
                    unsafe {
                        let _ = CloseHandle(event);
                    }
                    return;
                }
                if poke.swap(false, Ordering::SeqCst)
                    || (sonix_rf_alive(d.vid, d.pid, false)
                        && last_poke.elapsed() >= Duration::from_secs(15))
                {
                    unsafe {
                        poke_sonix_status(handle, d.output_len);
                    }
                    last_poke = Instant::now();
                }
                let w = unsafe { WaitForSingleObject(event, SONIX_WAIT_MS) };
                if w == WAIT_OBJECT_0 {
                    let mut n = 0u32;
                    unsafe {
                        let _ = GetOverlappedResult(handle, &ov, &mut n, false);
                    }
                    buf.truncate(n as usize);
                    out = Some(buf);
                    break;
                }
                if w != WAIT_TIMEOUT {
                    unsafe {
                        let _ = CancelIoEx(handle, Some(&ov as *const _));
                        let mut n = 0u32;
                        let _ = GetOverlappedResult(handle, &ov, &mut n, true);
                    }
                    unsafe {
                        let _ = CloseHandle(event);
                    }
                    return;
                }
                let up = sonix_rf_alive(d.vid, d.pid, false);
                if up != last_up {
                    last_up = up;
                    publish(last_raw, on_update);
                }
            }
            out
        };
        if let Some(bytes) = got {
            let n = SONIX_PKT_LOGS.fetch_add(1, Ordering::Relaxed);
            if n < 16 {
                crate::dblog::log(&format!(
                    "sonix pkt in{} batt={:?} {}",
                    bytes.len(),
                    parse_sonix_battery(&bytes),
                    to_hex(&bytes)
                ));
            }
            if let Some(pct) = parse_sonix_battery(&bytes) {
                let changed = last_pct != Some(pct);
                note_live_battery(d.vid, d.pid, pct);
                last_up = true;
                last_pct = Some(pct);
                if changed {
                    crate::dblog::log(&format!("sonix 0x0F battery={pct}%"));
                    publish(last_raw, on_update);
                }
            } else if !bytes.is_empty() {
                // Any live vendor report from the dongle means RF is up.
                if sonix_rf_alive(d.vid, d.pid, true) && !last_up {
                    last_up = true;
                    publish(last_raw, on_update);
                }
            }
        }
    }
    unsafe {
        let _ = CloseHandle(event);
    }
}

fn watch_via(
    handle: windows::Win32::Foundation::HANDLE,
    d: &HidDevice,
    parent_stop: &AtomicBool,
    child_stop: &AtomicBool,
    last_raw: &Mutex<Vec<HidDevice>>,
    on_update: &dyn Fn(Vec<Device>),
) {
    while !stopped(parent_stop, child_stop) {
        if let Some(pct) = crate::hid::query_eweadn_battery(handle, d.input_len, d.output_len) {
            note_live_battery(d.vid, d.pid, pct);
            crate::dblog::log(&format!("via 0x20 battery={pct}%"));
            publish(last_raw, on_update);
        } else if let Some(pct) = unsafe { query_via_battery(handle, d.input_len, d.output_len) } {
            note_live_battery(d.vid, d.pid, pct);
            crate::dblog::log(&format!("via 0xA4 battery={pct}%"));
            publish(last_raw, on_update);
        } else {
            let alive = unsafe { query_via_alive(handle, d.input_len, d.output_len) };
            let n = VIA_LOGS.fetch_add(1, Ordering::Relaxed);
            if n < 8 {
                crate::dblog::log(&format!("via ping alive={alive}"));
            }
            if alive && sonix_rf_alive(d.vid, d.pid, true) {
                publish(last_raw, on_update);
            }
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(12) && !stopped(parent_stop, child_stop) {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// HID++ 查询周期：接收器对特征请求的响应很快，30s 一次足够反映真实电量变化，
/// 也不会给无线链路增加可感知负担（失败会被 hidpp 内部的 DEAD_TTL 缓存挡掉）。
const HIDPP_PERIOD: Duration = Duration::from_secs(30);

/// 罗技 HID++：周期性地读 Unified Battery / Battery Status / Battery Level Status。
/// 成功即 note_live_battery（hidpp 内部完成）并立即重绘，与其它 2.4G 路径一致。
fn watch_hidpp(
    handle: windows::Win32::Foundation::HANDLE,
    d: &HidDevice,
    parent_stop: &AtomicBool,
    child_stop: &AtomicBool,
    last_raw: &Mutex<Vec<HidDevice>>,
    on_update: &dyn Fn(Vec<Device>),
) {
    while !stopped(parent_stop, child_stop) {
        if let Some(pct) =
            crate::hidpp::query_battery(handle, d.input_len, d.output_len, d.vid, d.pid)
        {
            crate::dblog::log(&format!("hidpp {:04X}:{:04X} battery={pct}%", d.vid, d.pid));
            publish(last_raw, on_update);
        }
        let start = Instant::now();
        while start.elapsed() < HIDPP_PERIOD && !stopped(parent_stop, child_stop) {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn watch_compx(
    handle: windows::Win32::Foundation::HANDLE,
    d: &HidDevice,
    parent_stop: &AtomicBool,
    child_stop: &AtomicBool,
    last_raw: &Mutex<Vec<HidDevice>>,
    on_update: &dyn Fn(Vec<Device>),
) {
    while !stopped(parent_stop, child_stop) {
        let got = unsafe { query_compx_battery(handle, d.input_len, d.output_len) };
        match got {
            Ok((pct, _)) => {
                note_live_battery(d.vid, d.pid, pct);
                publish(last_raw, on_update);
            }
            Err(_) => {}
        }
        let start = Instant::now();
        while start.elapsed() < COMPX_PERIOD && !stopped(parent_stop, child_stop) {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Block and print vendor packets for `seconds` (debug CLI).
pub fn listen(seconds: u64) {
    let (devs, _) = enumerate_ex(false);
    println!("listening {seconds}s on vendor collections…");
    let stop = Instant::now() + Duration::from_secs(seconds.max(1));
    let mut threads = Vec::new();
    for d in devs {
        if watch_kind(&d).is_none() {
            continue;
        }
        println!(
            "  {:04X}:{:04X} uP={:04X} in={} out={} {}",
            d.vid, d.pid, d.usage_page, d.input_len, d.output_len, d.product
        );
        threads.push(thread::spawn(move || unsafe {
            let Some((handle, _)) = open_hid(&d.path) else {
                println!("  open failed {:04X}:{:04X}", d.vid, d.pid);
                return;
            };
            let _ = HidD_SetNumInputBuffers(handle, 64);
            let in_len = (d.input_len as usize).max(1);
            while Instant::now() < stop {
                let remain = stop.saturating_duration_since(Instant::now());
                let ms = remain.as_millis().min(4000) as u32;
                if ms == 0 {
                    break;
                }
                match overlapped_read(handle, in_len, ms.max(50)) {
                    Ok(buf) => {
                        let hex = buf
                            .iter()
                            .map(|b| format!("{b:02X}"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let batt = parse_sonix_battery(&buf);
                        println!(
                            "  {:04X}:{:04X} in{} battery={:?} {}",
                            d.vid,
                            d.pid,
                            buf.len(),
                            batt,
                            hex
                        );
                    }
                    Err(e) if e.contains("timeout") => {}
                    Err(e) => {
                        println!("  {:04X}:{:04X} {e}", d.vid, d.pid);
                        break;
                    }
                }
            }
            let _ = CloseHandle(handle);
        }));
    }
    for t in threads {
        let _ = t.join();
    }
}
