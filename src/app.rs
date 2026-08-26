//! Main application: poll loop, bottom strip window, tray icon, context menu.
#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateFontW, CreateSolidBrush, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    EndPaint, FillRect, GetDC, GetDIBits, GetMonitorInfoW, GetTextExtentPoint32W, HBRUSH, HDC,
    HFONT, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    PAINTSTRUCT, ROP_CODE, ReleaseDC, ScreenToClient, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT, TextOutW,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW,
    RegSetValueExW,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_SYSTEM_AWARE, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RIDI_DEVICENAME, RegisterRawInputDevices,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFY_ICON_DATA_FLAGS, NOTIFYICONDATAW, Shell_NotifyIconW, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CheckMenuRadioItem, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowExW,
    FindWindowW, GA_ROOT, GWL_EXSTYLE, GWL_STYLE, GWLP_USERDATA, GetAncestor, GetClassNameW,
    GetClientRect, GetCursorPos, GetForegroundWindow, GetMessageW, GetParent, GetWindowLongPtrW,
    GetWindowRect, HICON, HMENU, HWND_NOTOPMOST, IDC_ARROW, IsIconic, IsWindowVisible, KillTimer,
    LAYERED_WINDOW_ATTRIBUTES_FLAGS, LWA_COLORKEY, LoadCursorW, MENU_ITEM_FLAGS, MF_BYCOMMAND,
    MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SET_WINDOW_POS_FLAGS, SPI_GETWORKAREA, SW_HIDE,
    SW_SHOWNA, SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SetForegroundWindow,
    SetLayeredWindowAttributes, SetParent, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    SystemParametersInfoW, TPM_RETURNCMD, TPM_RIGHTBUTTON, TRACK_POPUP_MENU_FLAGS, TrackPopupMenu,
    TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_DEVICECHANGE,
    WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_INPUT, WM_KILLFOCUS, WM_LBUTTONUP, WM_NULL, WM_PAINT,
    WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASS_STYLES, WNDCLASSEXW, WS_CHILD,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::btreader;
use crate::hid;
use crate::hidwatch;
use crate::icon;
use crate::model::{Device, Snapshot};
use crate::settings::Settings;
use crate::theme::{self, rgb};

#[link(name = "user32")]
unsafe extern "system" {
    fn PrintWindow(hwnd: HWND, hdcblt: HDC, nflags: u32) -> i32;
}

// ---------------------------------------------------------------------------
const TRAY_MSG: u32 = 0x8000 + 20;
/// Posted by the poll / HID watcher when a scan finishes so the strip repaints now.
const MSG_DATA_READY: u32 = 0x8000 + 21;
/// Posted to the helper window when the strip must be rebuilt after an
/// unexpected (Explorer-caused) destroy. Posted — not sent — so it is picked
/// up by GetMessage even while the old message stream is settling.
const MSG_RECREATE_STRIP: u32 = 0x8000 + 22;
const STRIP_TIMER: usize = 1;
const DEVICE_TIMER: usize = 2;
/// Helper-window timer used to retry a failed strip recreate without
/// blocking the UI thread.
const RECREATE_TIMER: usize = 3;
const RECREATE_RETRY_MS: u32 = 500;
const STRIP_HEIGHT: i32 = 40; // fallback floating height (no taskbar found)
const MIN_STRIP_HEIGHT: i32 = 24; // floor for degenerate taskbar rects
const MIN_STRIP_WIDTH: i32 = 40; // floor when clamping the strip width
const MARGIN_X: i32 = 6;
const DOT: i32 = 8;
const DOT_GAP: i32 = 6;
const NAME_PILL_GAP: i32 = 6;
const DEV_GAP: i32 = 16;
const PILL_PAD: i32 = 5;
const TASKBAR_RESERVE: i32 = 200; // px reserved for tray/clock when tray not measurable
const GAP_TRAY: i32 = 6; // gap between our strip and the tray cluster
const LEFT_INSET: i32 = 10;
static RAW_INPUT_LOGS: AtomicU32 = AtomicU32::new(0);

// overflow "+n" pill + full-device popup panel
const OVERFLOW_PILL_MIN: i32 = 44; // reserved width for the "+n" pill incl. gap
const PANEL_ROW_H: i32 = 26;
const PANEL_PAD: i32 = 10;
const PANEL_GAP_X: i32 = 12;

const MID_TOGGLE: usize = 1000;
const MID_REFRESH: usize = 1002;
const MID_INT10: usize = 1018;
const MID_INT30: usize = 1003;
const MID_INT60: usize = 1004;
const MID_INT300: usize = 1005;
const MID_AUTOSTART: usize = 1006;
const MID_NOTIFY: usize = 1007;
const MID_EXIT: usize = 1008;
const MID_SHOW_OFFLINE: usize = 1010;
const MID_POS_LEFT: usize = 1011;
const MID_POS_RIGHT: usize = 1012;
const MID_LOW10: usize = 1013;
const MID_LOW20: usize = 1014;
const MID_LOW30: usize = 1015;
const MID_BT_ADD: usize = 1016;
const MID_BT_DEVICES: usize = 1017;
const MID_DEV_BASE: usize = 3100;
const MAX_MENU_DEVICES: usize = 16;

// ---------------------------------------------------------------------------
pub struct App {
    hwnd: AtomicI64,
    raw_hwnd: AtomicI64,
    pub settings: Mutex<Settings>,
    snapshot: Mutex<Snapshot>,
    bt_devs: Mutex<Vec<Device>>,
    hid_devs: Mutex<Vec<Device>>,
    hid_watch: Mutex<Option<hidwatch::Watcher>>,
    dirty: AtomicBool,
    refresh_tx: Mutex<Option<Sender<()>>>,
    last_low: Mutex<HashMap<String, Instant>>,
    pending_alerts: Mutex<Vec<String>>,
    name_font: AtomicI64,
    pill_font: AtomicI64,
    tray_icon: Mutex<i64>,
    strip_visible: AtomicBool,
    /// Auto-hidden because a fullscreen app (e.g. fullscreen video) is active.
    fs_hidden: AtomicBool,
    demo_mode: AtomicBool,
    pub settings_addrs: Mutex<Vec<String>>,
    panel_hwnd: AtomicI64,
    overflow_x: Mutex<(i32, i32)>, // client-coord [left, right) of the "+n" pill
    layout: Mutex<StripLayout>,
    /// Current `Shell_TrayWnd` handle (0 when none / not discovered yet).
    taskbar_hwnd: AtomicI64,
    /// True once the strip is a real WS_CHILD of Shell_TrayWnd (SetParent done).
    embedded: AtomicBool,
    /// Registered "TaskbarCreated" message id (0 until registered).
    wm_taskbar_created: AtomicU32,
    /// Set when Explorer's death destroyed our (child) window: rebuild it
    /// after the current message settles instead of quitting.
    pending_recreate: AtomicBool,
    /// Set as soon as the user asks to quit (WM_CLOSE / menu Exit). Lets
    /// WM_DESTROY tell an explicit shutdown apart from Explorer killing our
    /// embedded child during a taskbar rebuild.
    shutting_down: AtomicBool,
    /// Guards against two overlapping recreate attempts on the UI thread.
    recreating: AtomicBool,
    /// How many recreate attempts failed in a row (for logging only).
    recreate_attempts: AtomicU32,
    /// Last placement we applied via SetWindowPos, to detect re-anchor needs.
    last_place: Mutex<StripPlace>,
}

/// Where the strip window currently sits, in whichever coordinate space is
/// relevant: taskbar-client coords when embedded, screen coords when floating.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct StripPlace {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// What the strip currently renders: the devices that fit, how many were cut,
/// and the resulting window width. Recomputed only when data/layout changes.
#[derive(Clone, Default)]
struct StripLayout {
    devices: Vec<Device>,
    overflow: usize,
    width: i32,
}

/// Whether the strip window should be shown or hidden right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StripVisibility {
    Shown,
    Hidden,
}

/// Pure visibility decision: the user's show/hide preference combined with
/// the fullscreen auto-hide suppression. This is the ONLY policy that may
/// request a ShowWindow/SWP_SHOWWINDOW for the main strip — embedding,
/// healing and recreating must never show a strip the user hid.
fn strip_visibility(user_pref_visible: bool, fs_autohidden: bool) -> StripVisibility {
    if user_pref_visible && !fs_autohidden {
        StripVisibility::Shown
    } else {
        StripVisibility::Hidden
    }
}

/// Pure decision for WM_DESTROY: an EMBEDDED strip destroyed while we are not
/// quitting means Explorer took the taskbar (and our child window) down.
/// Deliberately does NOT look at whether Shell_TrayWnd currently exists — a
/// freshly restarted Explorer may already have created a new one, and that
/// must not turn our rebuild into a silent exit.
fn should_schedule_rebuild_after_destroy(shutting_down: bool, was_embedded: bool) -> bool {
    !shutting_down && was_embedded
}

pub static APP: std::sync::OnceLock<Arc<App>> = std::sync::OnceLock::new();

impl App {
    pub fn new() -> Arc<Self> {
        let cfg = Settings::load();
        Arc::new(App {
            hwnd: AtomicI64::new(0),
            raw_hwnd: AtomicI64::new(0),
            settings: Mutex::new(cfg),
            snapshot: Mutex::new(Snapshot::default()),
            bt_devs: Mutex::new(Vec::new()),
            hid_devs: Mutex::new(Vec::new()),
            hid_watch: Mutex::new(None),
            dirty: AtomicBool::new(true),
            refresh_tx: Mutex::new(None),
            last_low: Mutex::new(HashMap::new()),
            pending_alerts: Mutex::new(Vec::new()),
            name_font: AtomicI64::new(0),
            pill_font: AtomicI64::new(0),
            tray_icon: Mutex::new(0),
            strip_visible: AtomicBool::new(true),
            fs_hidden: AtomicBool::new(false),
            demo_mode: AtomicBool::new(false),
            settings_addrs: Mutex::new(Vec::new()),
            panel_hwnd: AtomicI64::new(0),
            overflow_x: Mutex::new((0, 0)),
            layout: Mutex::new(StripLayout::default()),
            taskbar_hwnd: AtomicI64::new(0),
            embedded: AtomicBool::new(false),
            wm_taskbar_created: AtomicU32::new(0),
            pending_recreate: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            recreating: AtomicBool::new(false),
            recreate_attempts: AtomicU32::new(0),
            last_place: Mutex::new(StripPlace::default()),
        })
    }

    /// Fill snapshot with demo devices (visual preview / testing).
    pub fn enable_demo_data(&self) {
        use crate::model::Device;
        self.demo_mode.store(true, Ordering::Relaxed);
        let fake = vec![
            Device {
                name: "Keychron K3 Pro".to_string(),
                address: "AA:BB:CC:00:00:01".into(),
                is_le: true,
                connected: true,
                battery: Some(44),
                status: "ok",
                kind: "Keyboard",
            },
            Device {
                name: "Logitech MX Master 3S".to_string(),
                address: "AA:BB:CC:00:00:02".into(),
                is_le: true,
                connected: true,
                battery: Some(16),
                status: "ok",
                kind: "Mouse",
            },
            Device {
                name: "EDIFIER W820NB".to_string(),
                address: "AA:BB:CC:00:00:03".into(),
                is_le: true,
                connected: true,
                battery: Some(5),
                status: "ok",
                kind: "Headset",
            },
            Device {
                name: "Xbox 手柄".to_string(),
                address: "AA:BB:CC:00:00:04".into(),
                is_le: true,
                connected: false,
                battery: None,
                status: "offline",
                kind: "Gamepad",
            },
        ];
        let mut snap = self.snapshot.lock().unwrap();
        snap.devices = fake;
        snap.has_connected = true;
        self.dirty.store(true, Ordering::Release);
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd.load(Ordering::Relaxed) as _)
    }
    fn name_font(&self) -> HFONT {
        HFONT(self.name_font.load(Ordering::Relaxed) as _)
    }
    fn pill_font(&self) -> HFONT {
        HFONT(self.pill_font.load(Ordering::Relaxed) as _)
    }
    fn tray_hicon(&self) -> Option<HICON> {
        let v = *self.tray_icon.lock().unwrap();
        if v == 0 { None } else { Some(HICON(v as _)) }
    }

    pub fn live_devices(&self) -> Vec<crate::model::Device> {
        self.snapshot.lock().unwrap().devices.clone()
    }

    pub fn visible_devices(&self) -> Vec<crate::model::Device> {
        let devices = self.live_devices();
        self.settings.lock().unwrap().filter_for_strip(&devices)
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    pub fn request_refresh(&self) {
        if let Some(tx) = self.refresh_tx.lock().unwrap().as_ref() {
            let _ = tx.send(());
        }
        if let Some(w) = self.hid_watch.lock().unwrap().as_ref() {
            w.kick();
        }
    }

    pub fn publish_bt(&self, devs: Vec<Device>) {
        *self.bt_devs.lock().unwrap() = devs;
        self.rebuild_snapshot();
    }

    pub fn publish_hid(&self, devs: Vec<Device>) {
        *self.hid_devs.lock().unwrap() = devs;
        self.rebuild_snapshot();
    }

    /// A subscribed BLE device pushed a battery notification: patch the live
    /// BT rows and repaint immediately (no rescan needed).
    pub fn apply_notify_battery(&self, addr: &str, pct: u8) {
        let mut hit = false;
        {
            let mut bt = self.bt_devs.lock().unwrap();
            for d in bt.iter_mut() {
                if d.address == addr && d.connected {
                    d.battery = Some(pct);
                    d.status = "ok";
                    hit = true;
                }
            }
        }
        if hit {
            self.rebuild_snapshot();
        }
    }

    fn rebuild_snapshot(&self) {
        let mut devs = self.bt_devs.lock().unwrap().clone();
        devs.extend(self.hid_devs.lock().unwrap().clone());
        devs = crate::model::dedup_by_name(devs);
        devs.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let any_connected = devs.iter().any(|d| d.connected);
        {
            let mut cfg = self.settings.lock().unwrap();
            if cfg.remember(&devs) {
                cfg.save();
            }
        }
        self.queue_low_alerts(&devs);
        {
            let mut snap = self.snapshot.lock().unwrap();
            snap.devices = devs;
            snap.has_connected = any_connected;
        }
        self.dirty.store(true, Ordering::Release);
        let hwnd = self.hwnd();
        if !hwnd.0.is_null() {
            unsafe {
                let _ = PostMessageW(hwnd, MSG_DATA_READY, WPARAM(0), LPARAM(0));
            }
        }
    }

    fn queue_low_alerts(&self, devs: &[Device]) {
        let cfg = self.settings.lock().unwrap();
        if !cfg.notify_low {
            return;
        }
        let cooldown = Duration::from_secs(cfg.resend_minutes.saturating_mul(60));
        let threshold = cfg.low_battery_alert;
        let mut low = self.last_low.lock().unwrap();
        let mut alerts = Vec::new();
        let now = Instant::now();
        for d in devs {
            if !cfg.is_shown(&d.address) {
                continue;
            }
            if let Some(b) = d.battery
                && b <= threshold
            {
                let allow = low
                    .get(&d.address)
                    .map(|t| t.elapsed() >= cooldown)
                    .unwrap_or(true);
                if allow {
                    alerts.push(format!("{}：{}%", d.name, b));
                    low.insert(d.address.clone(), now);
                }
            }
        }
        drop(low);
        drop(cfg);
        if !alerts.is_empty() {
            self.pending_alerts.lock().unwrap().extend(alerts);
        }
    }

    pub fn apply_strip_visible(&self, show: bool) {
        self.strip_visible.store(show, Ordering::Relaxed);
        if !show {
            // Manual hide clears any fullscreen auto-hide state so the next
            // manual "show" always brings the strip back.
            self.fs_hidden.store(false, Ordering::Relaxed);
        }
        // No unconditional ShowWindow here: layout_and_resize() is the single
        // place that shows/hides, driven by strip_visibility(). This keeps a
        // user-hidden strip hidden across embeds, heals and recreates.
        self.heal_embedding();
        self.layout_and_resize();
    }

    /// Effective visibility: user preference AND not suppressed by a
    /// fullscreen app in the foreground.
    fn strip_should_show(&self) -> bool {
        strip_visibility(
            self.strip_visible.load(Ordering::Relaxed),
            self.fs_hidden.load(Ordering::Relaxed),
        ) == StripVisibility::Shown
    }

    /// Called once per second from the strip timer — ONLY for the degraded
    /// floating bar. An embedded strip lives inside the taskbar, so it can
    /// never cover a fullscreen app and needs no auto-hide.
    fn update_fullscreen_autohide(&self) {
        if self.embedded.load(Ordering::Relaxed) {
            return;
        }
        let pref_on = self.strip_visible.load(Ordering::Relaxed);
        if !pref_on {
            return; // user hid the strip themselves — nothing to manage
        }
        let fs = is_fullscreen_app_active(self.hwnd());
        let prev = self.fs_hidden.swap(fs, Ordering::Relaxed);
        if fs == prev {
            return;
        }
        unsafe {
            if fs {
                let _ = ShowWindow(self.hwnd(), SW_HIDE);
                let p = self.panel_hwnd.load(Ordering::Relaxed);
                if p != 0 {
                    let _ = ShowWindow(HWND(p as _), SW_HIDE);
                }
                crate::dblog::log("autohide: fullscreen app active, strip hidden");
            } else {
                let _ = ShowWindow(self.hwnd(), SW_SHOWNA);
                crate::dblog::log("autohide: fullscreen ended, strip restored");
            }
        }
    }

    // ==================================================================
    pub fn run(self: &Arc<Self>) -> i32 {
        unsafe {
            crate::dblog::log("run: start");
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE);
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            let class_name = w!("BtBatteryBarStrip");
            let hinst = HINSTANCE(GetModuleHandleW(PCWSTR::null()).unwrap().0);

            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: HICON::default(),
                hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap(),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: class_name,
                hIconSm: HICON::default(),
            };
            if RegisterClassExW(&wc) == 0 {
                crate::dblog::log("run: register class failed");
                return 1;
            }

            // The strip starts as a plain non-topmost popup; if a taskbar is
            // present it is converted to WS_CHILD + SetParent right below.
            // WS_EX_TOPMOST is intentionally never used: the bar must not
            // float above fullscreen video/games.
            let ex = WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0);
            let hwnd = CreateWindowExW(
                ex,
                class_name,
                w!("BtBatteryBar"),
                WS_POPUP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1,
                1,
                HWND::default(),
                HMENU::default(),
                hinst,
                None,
            )
            .unwrap();
            self.hwnd.store(hwnd.0 as i64, Ordering::Relaxed);
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, self.as_ref() as *const App as isize);

            self.name_font
                .store(create_font(-16, 600).0 as i64, Ordering::Relaxed);
            self.pill_font
                .store(create_font(-15, 700).0 as i64, Ordering::Relaxed);

            // Broadcast sent by Explorer whenever the taskbar is (re)created.
            let tbc = RegisterWindowMessageW(w!("TaskbarCreated"));
            self.wm_taskbar_created.store(tbc, Ordering::Relaxed);

            self.add_tray_icon();
            let _ = SetTimer(hwnd, STRIP_TIMER, 1000, None);
            self.apply_color_key();

            // Hidden top-level helper: raw-input sink AND the receiver for
            // TaskbarCreated / WM_DISPLAYCHANGE / WM_SETTINGCHANGE (a
            // WS_CHILD strip and HWND_MESSAGE windows never get those).
            let helper = create_helper_window(hinst, class_name);
            self.raw_hwnd.store(helper.0 as i64, Ordering::Relaxed);
            register_raw_keyboard(helper);

            // Publish the user's visibility preference BEFORE the first
            // heal/layout pass: embedding must never show a hidden strip.
            let pref_visible = self.settings.lock().unwrap().strip_visible;
            self.strip_visible.store(pref_visible, Ordering::Relaxed);

            // Prefer real embedding into Shell_TrayWnd; falls back to a
            // floating non-topmost popup when there is no taskbar.
            // layout_and_resize() applies visibility per strip_visibility().
            self.heal_embedding();
            self.layout_and_resize();

            if self.demo_mode.load(Ordering::Relaxed) {
                self.layout_and_resize();
                self.update_tray();
                self.paint();
                self.capture_self(&crate::dblog::path().with_file_name("strip-self.bmp"));
            } else {
                let (tx, rx) = channel::<()>();
                *self.refresh_tx.lock().unwrap() = Some(tx);
                {
                    let app = self.clone();
                    std::thread::Builder::new()
                        .name("bt-poll".into())
                        .spawn(move || poll_loop(app, rx))
                        .ok();
                }
                {
                    let app = self.clone();
                    let watcher = hidwatch::Watcher::start(Arc::new(move |devs| {
                        app.publish_hid(devs);
                    }));
                    *self.hid_watch.lock().unwrap() = Some(watcher);
                }
            }

            let mut msg = MSG::default();
            loop {
                let r = GetMessageW(&mut msg, HWND::default(), 0, 0);
                if r.0 == 0 || r.0 == -1 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
                // Primary recreate trigger is MSG_RECREATE_STRIP posted to
                // the helper window; this flag check is only a fallback in
                // case the helper was unavailable at schedule time.
                if self.pending_recreate.swap(false, Ordering::AcqRel) {
                    self.try_recreate_strip();
                }
            }

            // ---- normal-exit teardown (uses the CURRENT hwnd: the local
            // `hwnd` above is stale if the strip was recreated) ----
            self.shutting_down.store(true, Ordering::Release);
            let main = self.hwnd();
            if !main.0.is_null() {
                let _ = KillTimer(main, STRIP_TIMER);
                let _ = KillTimer(main, DEVICE_TIMER);
            }
            self.teardown_resources();
            self.remove_tray_icon();
            if let Some(h) = self.tray_hicon() {
                let _ = DeleteObject(HGDIOBJ(h.0));
            }
            let _ = DeleteObject(HGDIOBJ(self.name_font().0));
            let _ = DeleteObject(HGDIOBJ(self.pill_font().0));
            msg.wParam.0 as i32
        }
    }

    // ================================================================== tray
    fn notify_data(&self, icon: HICON, tip: &str) -> NOTIFYICONDATAW {
        self.notify_data_for(self.hwnd(), icon, tip)
    }

    fn notify_data_for(&self, hwnd: HWND, icon: HICON, tip: &str) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NOTIFY_ICON_DATA_FLAGS(NIF_MESSAGE.0 | NIF_ICON.0 | NIF_TIP.0);
        nid.uCallbackMessage = TRAY_MSG;
        nid.hIcon = icon;
        fill_wchars(&mut nid.szTip, tip, 128);
        nid
    }

    fn add_tray_icon(&self) {
        let icon = icon::make_icon(None).unwrap();
        *self.tray_icon.lock().unwrap() = icon.0 as i64;
        let nid = self.notify_data(icon, "蓝牙电量");
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    fn update_tray(&self) {
        let devices = self.visible_devices();
        let snap = Snapshot {
            devices: devices.clone(),
            scanned_at_ms: 0,
            has_connected: devices.iter().any(|d| d.connected),
        };
        let worst = worst_battery(&snap);
        let icon = icon::make_icon(worst).unwrap();
        {
            let mut slot = self.tray_icon.lock().unwrap();
            if *slot != 0 {
                unsafe {
                    let _ = DeleteObject(HGDIOBJ(*slot as *mut core::ffi::c_void));
                }
            }
            *slot = icon.0 as i64;
        }
        let tip = if snap.devices.is_empty() {
            "无设备".to_string()
        } else {
            snap.status_summary()
        };
        let nid = self.notify_data(icon, &tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    fn remove_tray_icon(&self) {
        let nid = self.notify_data(HICON::default(), "");
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }

    fn show_balloon(&self, title: &str, text: &str, warn: bool) {
        let icon = self.tray_hicon().unwrap_or_default();
        let mut nid = self.notify_data(icon, "");
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = if warn { NIIF_WARNING } else { NIIF_INFO };
        fill_wchars(&mut nid.szInfoTitle, title, 64);
        fill_wchars(&mut nid.szInfo, text, 256);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    // ================================================================== menu
    fn show_menu(&self) {
        unsafe {
            let Ok(root) = CreatePopupMenu() else { return };
            let Ok(m_display) = CreatePopupMenu() else {
                return;
            };
            let Ok(m_pos) = CreatePopupMenu() else { return };
            let Ok(m_dev) = CreatePopupMenu() else { return };
            let Ok(m_alert) = CreatePopupMenu() else {
                return;
            };
            let Ok(m_low) = CreatePopupMenu() else { return };
            let Ok(m_poll) = CreatePopupMenu() else {
                return;
            };

            let cfg = self.settings.lock().unwrap().clone();
            let live = self.live_devices();
            let rows = cfg.rows_for_settings(&live);
            let mut keep: Vec<Vec<u16>> = Vec::new();

            append_item(m_display, MID_TOGGLE, w!("显示电量条"), cfg.strip_visible);
            append_item(m_pos, MID_POS_LEFT, w!("靠左"), cfg.dock_left());
            append_item(m_pos, MID_POS_RIGHT, w!("靠右"), !cfg.dock_left());
            let _ = CheckMenuRadioItem(
                m_pos,
                MID_POS_LEFT as u32,
                MID_POS_RIGHT as u32,
                if cfg.dock_left() {
                    MID_POS_LEFT
                } else {
                    MID_POS_RIGHT
                } as u32,
                MF_BYCOMMAND.0,
            );
            append_popup(m_display, m_pos, w!("位置"));
            append_popup(root, m_display, w!("显示"));

            append_item(root, MID_REFRESH, w!("立即刷新"), false);

            append_item(
                m_dev,
                MID_SHOW_OFFLINE,
                w!("显示未连接 / 离线设备"),
                cfg.show_offline,
            );
            let _ = AppendMenuW(m_dev, MF_SEPARATOR, 0, PCWSTR::null());
            let mut addrs = Vec::new();
            if rows.is_empty() {
                append_flags(
                    m_dev,
                    MENU_ITEM_FLAGS(MF_STRING.0 | MF_GRAYED.0),
                    0,
                    "暂无设备",
                    &mut keep,
                );
            } else {
                for (i, row) in rows.iter().take(MAX_MENU_DEVICES).enumerate() {
                    let id = MID_DEV_BASE + i;
                    append_flags(m_dev, checked_flag(row.shown), id, &row.label(), &mut keep);
                    addrs.push(row.address.clone());
                }
            }
            *self.settings_addrs.lock().unwrap() = addrs;
            let _ = AppendMenuW(m_dev, MF_SEPARATOR, 0, PCWSTR::null());
            append_item(m_dev, MID_BT_ADD, w!("添加 / 连接蓝牙设备"), false);
            append_item(m_dev, MID_BT_DEVICES, w!("已连接的设备"), false);
            append_popup(root, m_dev, w!("设备"));

            append_item(m_alert, MID_NOTIFY, w!("低电量提醒"), cfg.notify_low);
            append_item(m_low, MID_LOW10, w!("10%"), cfg.low_battery_alert == 10);
            append_item(m_low, MID_LOW20, w!("20%"), cfg.low_battery_alert == 20);
            append_item(m_low, MID_LOW30, w!("30%"), cfg.low_battery_alert == 30);
            let low_id = match cfg.low_battery_alert {
                10 => MID_LOW10,
                30 => MID_LOW30,
                _ => MID_LOW20,
            };
            let _ = CheckMenuRadioItem(
                m_low,
                MID_LOW10 as u32,
                MID_LOW30 as u32,
                low_id as u32,
                MF_BYCOMMAND.0,
            );
            append_popup(m_alert, m_low, w!("阈值"));
            append_popup(root, m_alert, w!("提醒"));

            append_item(m_poll, MID_INT10, w!("10 秒"), cfg.poll_seconds == 10);
            append_item(m_poll, MID_INT30, w!("30 秒"), cfg.poll_seconds == 30);
            append_item(m_poll, MID_INT60, w!("1 分钟"), cfg.poll_seconds == 60);
            append_item(m_poll, MID_INT300, w!("5 分钟"), cfg.poll_seconds == 300);
            let poll_id = match cfg.poll_seconds {
                10 => MID_INT10,
                30 => MID_INT30,
                300 => MID_INT300,
                _ => MID_INT60,
            };
            let _ = CheckMenuRadioItem(
                m_poll,
                MID_INT10 as u32,
                MID_INT300 as u32,
                poll_id as u32,
                MF_BYCOMMAND.0,
            );
            append_popup(root, m_poll, w!("轮询间隔"));

            append_item(root, MID_AUTOSTART, w!("开机自启动"), cfg.autostart);
            let _ = AppendMenuW(root, MF_SEPARATOR, 0, PCWSTR::null());
            append_item(root, MID_EXIT, w!("退出"), false);

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Menus need a foreground owner to dismiss on outside clicks.
            // When embedded, our strip is a WS_CHILD, so foreground goes to
            // its root ancestor (the taskbar) instead of the child itself.
            let root_hwnd = GetAncestor(self.hwnd(), GA_ROOT);
            if !root_hwnd.0.is_null() {
                let _ = SetForegroundWindow(root_hwnd);
            }
            let ret = TrackPopupMenu(
                root,
                TRACK_POPUP_MENU_FLAGS(TPM_RIGHTBUTTON.0 | TPM_RETURNCMD.0),
                pt.x,
                pt.y,
                0,
                self.hwnd(),
                None,
            );
            let _ = DestroyMenu(root);
            let _ = keep;
            // TrackPopupMenu enters its own modal loop; post a wake-up so the
            // outer message loop reliably picks up anything queued while the
            // menu was open (dismiss-on-click etc.).
            let _ = PostMessageW(self.hwnd(), WM_NULL, WPARAM(0), LPARAM(0));
            if ret.0 != 0 {
                self.on_command(ret.0 as usize);
            }
        }
    }

    fn on_command(&self, id: usize) {
        match id {
            MID_TOGGLE => {
                let show = {
                    let mut cfg = self.settings.lock().unwrap();
                    cfg.strip_visible = !cfg.strip_visible;
                    let v = cfg.strip_visible;
                    cfg.save();
                    v
                };
                self.apply_strip_visible(show);
            }
            MID_POS_LEFT | MID_POS_RIGHT => {
                {
                    let mut cfg = self.settings.lock().unwrap();
                    cfg.strip_side = if id == MID_POS_LEFT { "left" } else { "right" }.into();
                    cfg.save();
                }
                self.mark_dirty();
            }
            MID_REFRESH => self.request_refresh(),
            MID_INT10 | MID_INT30 | MID_INT60 | MID_INT300 => {
                let v = if id == MID_INT10 {
                    10
                } else if id == MID_INT30 {
                    30
                } else if id == MID_INT300 {
                    300
                } else {
                    60
                };
                let mut cfg = self.settings.lock().unwrap();
                cfg.poll_seconds = v;
                cfg.save();
                drop(cfg);
                self.request_refresh();
            }
            MID_AUTOSTART => {
                let on = {
                    let mut cfg = self.settings.lock().unwrap();
                    cfg.autostart = !cfg.autostart;
                    let v = cfg.autostart;
                    cfg.save();
                    v
                };
                set_autostart(on);
            }
            MID_NOTIFY => {
                let mut cfg = self.settings.lock().unwrap();
                cfg.notify_low = !cfg.notify_low;
                cfg.save();
            }
            MID_SHOW_OFFLINE => {
                let mut cfg = self.settings.lock().unwrap();
                cfg.show_offline = !cfg.show_offline;
                cfg.save();
                drop(cfg);
                self.mark_dirty();
            }
            MID_LOW10 | MID_LOW20 | MID_LOW30 => {
                let v = if id == MID_LOW10 {
                    10
                } else if id == MID_LOW30 {
                    30
                } else {
                    20
                };
                let mut cfg = self.settings.lock().unwrap();
                cfg.low_battery_alert = v;
                cfg.save();
                drop(cfg);
                self.mark_dirty();
            }
            MID_BT_ADD => open_ms_settings("ms-settings:bluetooth"),
            MID_BT_DEVICES => open_ms_settings("ms-settings:connecteddevices"),
            MID_EXIT => unsafe {
                // Mark the explicit quit FIRST so WM_DESTROY can tell it
                // apart from an Explorer-caused destroy, then take the same
                // path as WM_CLOSE.
                self.shutting_down.store(true, Ordering::Release);
                let _ = DestroyWindow(self.hwnd());
            },
            id if (MID_DEV_BASE..MID_DEV_BASE + MAX_MENU_DEVICES).contains(&id) => {
                let idx = id - MID_DEV_BASE;
                let addrs = self.settings_addrs.lock().unwrap().clone();
                if let Some(addr) = addrs.get(idx) {
                    let mut cfg = self.settings.lock().unwrap();
                    let shown = cfg.is_shown(addr);
                    cfg.set_shown(addr, !shown);
                    cfg.save();
                }
                self.mark_dirty();
            }
            _ => {}
        }
    }

    fn apply_color_key(&self) {
        unsafe {
            let _ = SetLayeredWindowAttributes(
                self.hwnd(),
                COLORREF(theme::CHROMA),
                255,
                LAYERED_WINDOW_ATTRIBUTES_FLAGS(LWA_COLORKEY.0),
            );
        }
    }

    /// Legacy compatibility hook (the old `topmost` setting / menu entry is
    /// gone). Embedding made global TopMost unnecessary and harmful — it made
    /// the bar float above fullscreen video — so this now ONLY ever clears
    /// TopMost on the degraded floating bar and never raises any window.
    #[allow(dead_code)]
    pub fn apply_topmost(&self, _top: bool) {
        if self.embedded.load(Ordering::Relaxed) {
            return; // embedded children must never touch z-order globally
        }
        unsafe {
            let _ = SetWindowPos(
                self.hwnd(),
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SET_WINDOW_POS_FLAGS(SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0),
            );
        }
    }

    // ================================================================== embedding
    /// Self-healing re-anchor, called every second and on TaskbarCreated /
    /// WM_DISPLAYCHANGE / WM_SETTINGCHANGE:
    /// - no taskbar          -> detach to a floating non-topmost popup
    /// - taskbar + no parent -> (re-)embed via WS_CHILD + SetParent
    /// - wrong parent        -> detach and re-embed into the current one
    fn heal_embedding(&self) {
        let hwnd = self.hwnd();
        if hwnd.0.is_null() {
            return;
        }
        unsafe {
            let shell = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).unwrap_or_default();
            if shell.0.is_null() {
                if self.embedded.swap(false, Ordering::Relaxed) {
                    self.detach_to_floating();
                }
                self.taskbar_hwnd.store(0, Ordering::Relaxed);
                return;
            }
            self.taskbar_hwnd.store(shell.0 as i64, Ordering::Relaxed);
            let parent_ok = self.embedded.load(Ordering::Relaxed)
                && GetParent(hwnd).map(|p| p == shell).unwrap_or(false);
            if parent_ok {
                return; // healthy: still parented to the live taskbar
            }
            if self.embedded.load(Ordering::Relaxed) {
                // Parented to something stale: detach before re-parenting.
                let _ = SetParent(hwnd, HWND::default());
                self.embedded.store(false, Ordering::Relaxed);
            }
            if !self.embed_into_taskbar(shell) && self.strip_should_show() {
                // Degraded mode: make sure the floating bar is visible (only
                // when the visibility policy says so — never for a user-
                // hidden strip).
                let _ = ShowWindow(hwnd, SW_SHOWNA);
            }
        }
    }

    /// Switch the strip to WS_CHILD, drop WS_EX_TOPMOST and SetParent it into
    /// `shell` (Shell_TrayWnd). Returns false when SetParent failed — the
    /// caller then keeps the strip as an independent non-topmost popup.
    unsafe fn embed_into_taskbar(&self, shell: HWND) -> bool {
        let hwnd = self.hwnd();
        if hwnd.0.is_null() || shell.0.is_null() {
            return false;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let new_style = (style & !WS_POPUP.0) | WS_CHILD.0;
        if new_style != style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, new_style as isize);
        }
        // Never topmost — neither floating nor embedded.
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let new_ex = ex & !WS_EX_TOPMOST.0;
        if new_ex != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex as isize);
        }
        match SetParent(hwnd, shell) {
            Ok(_) => {
                self.embedded.store(true, Ordering::Relaxed);
                self.taskbar_hwnd.store(shell.0 as i64, Ordering::Relaxed);
                // An embedded strip can't cover fullscreen apps; clear any
                // stale auto-hide from a previous degraded session.
                self.fs_hidden.store(false, Ordering::Relaxed);
                crate::dblog::log("taskbar: embedded into Shell_TrayWnd (WS_CHILD + SetParent)");
                // Visibility is decided by layout_and_resize() via
                // strip_visibility() — a user-hidden strip stays hidden.
                self.layout_and_resize();
                true
            }
            Err(e) => {
                crate::dblog::log(&format!(
                    "taskbar: SetParent failed ({e}); degrading to floating non-topmost bar"
                ));
                // Restore popup style so the fallback behaves like before.
                SetWindowLongPtrW(
                    hwnd,
                    GWL_STYLE,
                    ((new_style & !WS_CHILD.0) | WS_POPUP.0) as isize,
                );
                false
            }
        }
    }

    /// Leave the (dying or dead) taskbar: undo WS_CHILD/SetParent and become
    /// an independent WS_POPUP again — deliberately WITHOUT WS_EX_TOPMOST so
    /// fullscreen video/games stay uncovered.
    unsafe fn detach_to_floating(&self) {
        let hwnd = self.hwnd();
        if hwnd.0.is_null() {
            return;
        }
        let _ = SetParent(hwnd, HWND::default());
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            ((style & !WS_CHILD.0) | WS_POPUP.0) as isize,
        );
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_TOPMOST.0) as isize);
        self.taskbar_hwnd.store(0, Ordering::Relaxed);
        crate::dblog::log("taskbar: detached to floating non-topmost mode");
        self.layout_and_resize();
    }

    /// Kick off the recreate pipeline after an unexpected destroy. Safe to
    /// call from inside WM_DESTROY: it only sets a flag and POSTS a message
    /// to the still-alive helper window, so the actual rebuild happens later
    /// on the UI thread instead of starving inside GetMessage.
    fn schedule_recreate(&self) {
        self.pending_recreate.store(true, Ordering::Release);
        self.recreate_attempts.store(0, Ordering::Relaxed);
        let helper = HWND(self.raw_hwnd.load(Ordering::Relaxed) as _);
        if !helper.0.is_null() {
            unsafe {
                let _ = PostMessageW(helper, MSG_RECREATE_STRIP, WPARAM(0), LPARAM(0));
            }
        }
        // If the helper is somehow gone, the message loop's pending_recreate
        // fallback check still triggers a rebuild on the next dispatched
        // message.
    }

    /// One recreate attempt on the UI thread. On failure it arms a
    /// helper-window timer to retry later — it NEVER sleeps the UI thread.
    fn try_recreate_strip(&self) {
        if self.shutting_down.load(Ordering::Acquire) {
            self.pending_recreate.store(false, Ordering::Release);
            return;
        }
        if self.hwnd.load(Ordering::Relaxed) != 0 {
            // Spurious trigger while a strip already exists: nothing to do.
            self.pending_recreate.store(false, Ordering::Release);
            return;
        }
        // Guard against overlapping attempts (message + timer can queue up).
        if self.recreating.swap(true, Ordering::AcqRel) {
            return;
        }
        match unsafe { self.create_strip_window_once() } {
            Ok(h) => {
                self.recreate_attempts.store(0, Ordering::Relaxed);
                self.pending_recreate.store(false, Ordering::Release);
                unsafe { self.finish_recreate(h) };
            }
            Err(e) => {
                let n = self.recreate_attempts.fetch_add(1, Ordering::Relaxed) + 1;
                if n == 1 || n % 10 == 0 {
                    crate::dblog::log(&format!(
                        "taskbar: recreate attempt {n} failed ({e}); retrying via helper timer"
                    ));
                }
                let helper = HWND(self.raw_hwnd.load(Ordering::Relaxed) as _);
                if !helper.0.is_null() {
                    unsafe {
                        let _ = SetTimer(helper, RECREATE_TIMER, RECREATE_RETRY_MS, None);
                    }
                }
                // Keep pending_recreate set so the message-loop fallback can
                // also fire if the helper dies before the timer does.
            }
        }
        self.recreating.store(false, Ordering::Release);
    }

    /// Single CreateWindow attempt for the strip — no retries, no sleeping.
    unsafe fn create_strip_window_once(&self) -> windows::core::Result<HWND> {
        const CLASS_NAME: PCWSTR = w!("BtBatteryBarStrip");
        let hinst = HINSTANCE(GetModuleHandleW(PCWSTR::null()).unwrap().0);
        let ex = WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_LAYERED.0);
        CreateWindowExW(
            ex,
            CLASS_NAME,
            w!("BtBatteryBar"),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1,
            1,
            HWND::default(),
            HMENU::default(),
            hinst,
            None,
        )
    }

    /// Wire up a freshly created strip window after an unexpected destroy.
    unsafe fn finish_recreate(&self, hwnd: HWND) {
        crate::dblog::log("taskbar: strip window recreated");
        self.hwnd.store(hwnd.0 as i64, Ordering::Relaxed);
        let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, self as *const App as isize);
        let _ = SetTimer(hwnd, STRIP_TIMER, 1000, None);
        self.apply_color_key();
        // Click-hit ranges from the dead window must not survive.
        *self.overflow_x.lock().unwrap() = (0, 0);
        self.add_tray_icon();
        // Applies visibility strictly per strip_visibility(): a strip the
        // user hid stays hidden after the Explorer rebuild.
        self.layout_and_resize();
        self.heal_embedding();
        crate::dblog::log("taskbar: recreate complete");
    }

    /// Close and destroy the overflow device panel, if one exists. Needed on
    /// unexpected destroys: the panel is an independent topmost popup that
    /// would otherwise linger on screen with no strip left to dismiss it.
    fn destroy_panel(&self) {
        let p = self.panel_hwnd.swap(0, Ordering::Relaxed);
        if p != 0 {
            unsafe {
                let _ = DestroyWindow(HWND(p as _));
            }
        }
    }

    /// Release everything the app owns on normal exit (raw/helper sink,
    /// overflow panel, poll channel, HID watcher). Idempotent: WM_DESTROY
    /// runs it and the post-loop cleanup in run() may run it again.
    fn teardown_resources(&self) {
        unregister_raw_keyboard();
        let raw = HWND(self.raw_hwnd.swap(0, Ordering::Relaxed) as _);
        if !raw.0.is_null() {
            unsafe {
                let _ = DestroyWindow(raw);
            }
        }
        self.destroy_panel();
        *self.hid_watch.lock().unwrap() = None;
        *self.refresh_tx.lock().unwrap() = None;
    }

    // ================================================================== layout
    fn layout_and_resize(&self) {
        let layout = self.compute_layout();
        let width = layout.width;
        *self.layout.lock().unwrap() = layout;
        let place = self.desired_placement(width);
        *self.last_place.lock().unwrap() = place;
        // Single visibility policy for show AND hide — embedding, healing
        // and recreating all funnel through here.
        let visible = strip_visibility(
            self.strip_visible.load(Ordering::Relaxed),
            self.fs_hidden.load(Ordering::Relaxed),
        ) == StripVisibility::Shown;
        unsafe {
            let hwnd = self.hwnd();
            let mut flags =
                SET_WINDOW_POS_FLAGS(SWP_NOZORDER.0 | SWP_NOACTIVATE.0 | SWP_FRAMECHANGED.0);
            if visible {
                flags = SET_WINDOW_POS_FLAGS(flags.0 | SWP_SHOWWINDOW.0);
            }
            // Embedded: x/y are in the taskbar's client coords. Floating:
            // screen coords. Both handled by desired_placement().
            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                place.x,
                place.y,
                place.w,
                place.h,
                flags,
            );
            if !visible {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    /// Compute where the strip should be right now.
    /// - embedded: pure math against the taskbar's own client rect
    /// - degraded with a taskbar: overlay it in SCREEN coords (non-topmost)
    /// - no taskbar at all: dock to the work area bottom
    fn desired_placement(&self, width: i32) -> StripPlace {
        let dock_left = self.settings.lock().unwrap().dock_left();
        if self.embedded.load(Ordering::Relaxed) {
            if let Some((tb_client, tray_left)) = taskbar_client_geometry() {
                let (x, y, w, h) =
                    strip_placement_in_taskbar(tb_client, tray_left, dock_left, width);
                return StripPlace { x, y, w, h };
            }
            // Geometry hiccup while flagged embedded: stay put this tick.
            return *self.last_place.lock().unwrap();
        }
        unsafe {
            match find_taskbar() {
                Some((_shell, tb, tray_left)) => {
                    let (x, y, h) = strip_placement_on_screen(tb, tray_left, dock_left, width);
                    StripPlace { x, y, w: width, h }
                }
                None => {
                    let mut wa = RECT::default();
                    let _ = SystemParametersInfoW(
                        SPI_GETWORKAREA,
                        0,
                        Some(&mut wa as *mut _ as *mut _),
                        SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                    );
                    let x = if dock_left {
                        wa.left + LEFT_INSET
                    } else {
                        wa.right - width - 6
                    };
                    StripPlace {
                        x,
                        y: wa.bottom - STRIP_HEIGHT - 6,
                        w: width,
                        h: STRIP_HEIGHT,
                    }
                }
            }
        }
    }

    /// Measure devices against the taskbar width budget. Devices that no
    /// longer fit are counted into `overflow` and shown as a "+n" pill that
    /// opens the full-device popup panel.
    fn compute_layout(&self) -> StripLayout {
        unsafe {
            let dc = GetDC(HWND::default());
            let all = self.visible_devices();
            let max_w = max_strip_width();
            let mut chosen: Vec<Device> = Vec::new();
            let mut w = 0i32;
            let mut overflow = 0usize;
            for (i, d) in all.iter().enumerate() {
                let oldn = SelectObject(dc, HGDIOBJ(self.name_font().0));
                let (tw, _) = text_size(dc, &d.name);
                let _ = SelectObject(dc, oldn);
                let oldp = SelectObject(dc, HGDIOBJ(self.pill_font().0));
                let (pw, _) = text_size(dc, &pill_text(&d));
                let _ = SelectObject(dc, oldp);
                let item_w = if i == 0 { 0 } else { DEV_GAP }
                    + DOT
                    + DOT_GAP
                    + tw
                    + NAME_PILL_GAP
                    + pw
                    + PILL_PAD * 2;
                let more_after = all.len() - i > 1;
                // Reserve room for the "+n" pill whenever something might be cut.
                let reserve = if more_after { OVERFLOW_PILL_MIN } else { 0 };
                if w + item_w + reserve > max_w - MARGIN_X * 2 {
                    overflow = all.len() - i;
                    break;
                }
                w += item_w;
                chosen.push(d.clone());
            }
            if overflow > 0 {
                w += DEV_GAP + OVERFLOW_PILL_MIN;
            }
            let _ = ReleaseDC(HWND::default(), dc);
            StripLayout {
                devices: chosen,
                overflow,
                width: (w + MARGIN_X * 2).max(80).min(max_w),
            }
        }
    }

    fn cached_layout(&self) -> StripLayout {
        self.layout.lock().unwrap().clone()
    }

    /// Idle-tick re-anchor: verify the taskbar hasn't moved/resized.
    /// Self-healing (heal_embedding) and fullscreen auto-hide are handled
    /// once per tick in on_timer() — NOT here — so a data-ready message and
    /// the strip timer can't run them twice in the same tick.
    fn reanchor_if_needed(&self) {
        if !self.strip_visible.load(Ordering::Relaxed) {
            return;
        }
        if self.embedded.load(Ordering::Relaxed) && self.fs_hidden.swap(false, Ordering::Relaxed) {
            // Stale fullscreen-hide flag on an embedded strip: embedded
            // windows live inside the taskbar and are never covered.
            unsafe {
                let _ = ShowWindow(self.hwnd(), SW_SHOWNA);
            }
        }
        let width = self.cached_layout().width.max(80);
        let want = self.desired_placement(width);
        if *self.last_place.lock().unwrap() != want {
            self.layout_and_resize();
        }
    }

    // ================================================================== paint
    fn paint(&self) {
        unsafe {
            let hwnd = self.hwnd();
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);

            let md = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let oldbmp = SelectObject(md, HGDIOBJ(bmp.0));

            let theme = theme::detect();

            let bg = CreateSolidBrush(COLORREF(theme::CHROMA));
            let _ = FillRect(md, &rc, bg);
            let _ = DeleteObject(HGDIOBJ(bg.0));

            let layout = self.cached_layout();
            let devices = &layout.devices;
            let low_alert = self.settings.lock().unwrap().low_battery_alert;
            let cy = (rc.bottom - rc.top) / 2;

            if devices.is_empty() {
                let old = SelectObject(md, HGDIOBJ(self.name_font().0));
                let (_, th) = text_size(md, "暂无设备");
                draw_text_halo(
                    md,
                    MARGIN_X,
                    cy - th / 2,
                    "暂无设备",
                    theme.fg,
                    theme.fg_outline,
                );
                let _ = SelectObject(md, old);
            } else {
                let mut cx = MARGIN_X;
                for (i, d) in devices.iter().enumerate() {
                    cx += if i == 0 { 0 } else { DEV_GAP };

                    let dcol = if !d.connected {
                        theme.dot_off
                    } else if d.battery.map(|b| b <= low_alert).unwrap_or(false) {
                        theme.dot_low
                    } else {
                        theme.dot_ok
                    };
                    let dbr = CreateSolidBrush(COLORREF(dcol));
                    let drect = RECT {
                        left: cx,
                        top: cy - DOT / 2,
                        right: cx + DOT,
                        bottom: cy + DOT / 2 + 1,
                    };
                    let _ = FillRect(md, &drect, dbr);
                    let _ = DeleteObject(HGDIOBJ(dbr.0));
                    cx += DOT + DOT_GAP;

                    let old = SelectObject(md, HGDIOBJ(self.name_font().0));
                    let (tw, th) = text_size(md, &d.name);
                    draw_text_halo(md, cx, cy - th / 2, &d.name, theme.fg, theme.fg_outline);
                    let _ = SelectObject(md, old);
                    cx += tw + NAME_PILL_GAP;

                    let (pcolor, pfg) = match d.battery {
                        Some(v) if v <= low_alert => (theme.pill_low, rgb(255, 255, 255)),
                        Some(v) if v <= 50 => (theme.pill_mid, rgb(255, 255, 255)),
                        Some(_) => (theme.pill_ok, rgb(255, 255, 255)),
                        None => (theme.pill_off_bg, theme.pill_off_fg),
                    };
                    let ptext = pill_text(&d);
                    let old2 = SelectObject(md, HGDIOBJ(self.pill_font().0));
                    let (pw, ph) = text_size(md, &ptext);
                    let pbr = CreateSolidBrush(COLORREF(pcolor));
                    let prect = RECT {
                        left: cx,
                        top: cy - ph / 2 - 2,
                        right: cx + pw + PILL_PAD * 2,
                        bottom: cy + ph / 2 + 2,
                    };
                    let _ = FillRect(md, &prect, pbr);
                    let _ = DeleteObject(HGDIOBJ(pbr.0));
                    draw_text_halo(
                        md,
                        cx + PILL_PAD,
                        cy - ph / 2,
                        &ptext,
                        pfg,
                        theme.fg_outline,
                    );
                    let _ = SelectObject(md, old2);
                    cx += pw + PILL_PAD * 2;
                }

                // "+n" pill: remaining devices that did not fit. Clicking it
                // opens the full-device popup panel.
                if layout.overflow > 0 {
                    cx += DEV_GAP;
                    let text = format!("+{}", layout.overflow);
                    let old3 = SelectObject(md, HGDIOBJ(self.pill_font().0));
                    let (pw, ph) = text_size(md, &text);
                    let pill_w = pw + PILL_PAD * 2;
                    let pbr = CreateSolidBrush(COLORREF(theme.pill_off_bg));
                    let prect = RECT {
                        left: cx,
                        top: cy - ph / 2 - 2,
                        right: cx + pill_w,
                        bottom: cy + ph / 2 + 2,
                    };
                    let _ = FillRect(md, &prect, pbr);
                    let _ = DeleteObject(HGDIOBJ(pbr.0));
                    draw_text_halo(
                        md,
                        cx + PILL_PAD,
                        cy - ph / 2,
                        &text,
                        rgb(255, 255, 255),
                        theme.fg_outline,
                    );
                    let _ = SelectObject(md, old3);
                    *self.overflow_x.lock().unwrap() = (cx, cx + pill_w);
                }
            }

            let _ = BitBlt(
                hdc,
                0,
                0,
                rc.right,
                rc.bottom,
                md,
                0,
                0,
                ROP_CODE(0x00CC0020),
            );
            let _ = SelectObject(md, oldbmp);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(md);
            let _ = EndPaint(hwnd, &ps);
        }
    }

    // ==================================================================
    fn on_timer(&self) {
        // Cheap check first: fullscreen detection costs one foreground query.
        self.update_fullscreen_autohide();
        // Heal EVERY tick — even when the data is not dirty and even when the
        // user hid the strip. This keeps the Explorer self-healing state
        // fresh so a later "show" from the tray works immediately.
        self.heal_embedding();
        self.process_dirty_or_reanchor();
    }

    /// Repaint/relayout when new data arrived, else do the idle re-anchor.
    fn process_dirty_or_reanchor(&self) {
        if self.dirty.swap(false, Ordering::Acquire) {
            self.layout_and_resize();
            self.update_tray();
            unsafe {
                let hwnd = self.hwnd();
                let _ = InvalidateRect(hwnd, None, false);
            }
            if self.panel_hwnd.load(Ordering::Relaxed) != 0 {
                self.size_and_paint_panel();
            }
            self.drain_alerts();
        } else {
            self.reanchor_if_needed();
        }
    }

    // ================================================= overflow device panel
    /// Full list shown in the popup panel: everything that passes the
    /// show/hide + offline filter, including devices cut from the strip.
    fn panel_devices(&self) -> Vec<Device> {
        self.visible_devices()
    }

    fn toggle_panel(&self) {
        if self.panel_hwnd.load(Ordering::Relaxed) != 0
            && unsafe { IsWindowVisible(HWND(self.panel_hwnd.load(Ordering::Relaxed) as _)) }
                .as_bool()
        {
            self.hide_panel();
        } else {
            self.show_panel();
        }
    }

    fn hide_panel(&self) {
        let h = self.panel_hwnd.load(Ordering::Relaxed);
        if h != 0 {
            unsafe {
                let _ = ShowWindow(HWND(h as _), SW_HIDE);
            }
        }
    }

    fn show_panel(&self) {
        let hwnd = self.ensure_panel();
        if hwnd.0.is_null() {
            return;
        }
        self.size_and_paint_panel();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    fn ensure_panel(&self) -> HWND {
        let existing = self.panel_hwnd.load(Ordering::Relaxed);
        if existing != 0 {
            return HWND(existing as _);
        }
        unsafe {
            let class_name = w!("BtBatteryBarPanel");
            let hinst = HINSTANCE(GetModuleHandleW(PCWSTR::null()).unwrap().0);
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: WNDCLASS_STYLES(0),
                lpfnWndProc: Some(panel_wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: HICON::default(),
                hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: class_name,
                hIconSm: HICON::default(),
            };
            // ERROR_CLASS_ALREADY_EXISTS on second call is fine.
            let _ = RegisterClassExW(&wc);
            let ex = WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_LAYERED.0);
            // For a WS_POPUP window this parameter is the *owner*, not the
            // parent — the panel must stay an independent top-level popup,
            // never a taskbar/strip child. When the strip itself is an
            // embedded WS_CHILD it cannot be an owner, so pass none.
            let owner = if self.embedded.load(Ordering::Relaxed) {
                HWND::default()
            } else {
                self.hwnd()
            };
            let hwnd = CreateWindowExW(
                ex,
                class_name,
                w!("BtBatteryBarPanel"),
                WS_POPUP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1,
                1,
                owner,
                HMENU::default(),
                hinst,
                None,
            );
            match hwnd {
                Ok(h) => {
                    self.panel_hwnd.store(h.0 as i64, Ordering::Relaxed);
                    let _ = SetLayeredWindowAttributes(
                        h,
                        COLORREF(theme::CHROMA),
                        255,
                        LAYERED_WINDOW_ATTRIBUTES_FLAGS(LWA_COLORKEY.0),
                    );
                    h
                }
                Err(_) => HWND::default(),
            }
        }
    }

    /// Resize the panel to fit its rows and repaint (also re-anchors it just
    /// above the strip).
    fn size_and_paint_panel(&self) {
        let hwnd = HWND(self.panel_hwnd.load(Ordering::Relaxed) as _);
        if hwnd.0.is_null() {
            return;
        }
        let devices = self.panel_devices();
        let (w, h) = self.measure_panel(&devices);
        unsafe {
            let mut sr = RECT::default();
            let _ = GetWindowRect(self.hwnd(), &mut sr);
            let mut wa = RECT::default();
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut wa as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            let x = (sr.right - w - 2).max(wa.left + 4);
            let mut y = sr.top - h - GAP_TRAY;
            if y < wa.top {
                y = (sr.bottom + GAP_TRAY).min(wa.bottom - h - 4);
            }
            let _ = SetWindowPos(
                hwnd,
                HWND::default(),
                x,
                y,
                w,
                h,
                SET_WINDOW_POS_FLAGS(SWP_NOZORDER.0 | SWP_NOACTIVATE.0),
            );
            let _ = InvalidateRect(hwnd, None, false);
        }
    }

    fn measure_panel(&self, devices: &[Device]) -> (i32, i32) {
        unsafe {
            let dc = GetDC(HWND::default());
            let mut name_w = 60i32;
            let mut pill_w = 40i32;
            for d in devices {
                let oldn = SelectObject(dc, HGDIOBJ(self.name_font().0));
                let (tw, _) = text_size(dc, &d.name);
                let _ = SelectObject(dc, oldn);
                let oldp = SelectObject(dc, HGDIOBJ(self.pill_font().0));
                let (pw, _) = text_size(dc, &pill_text(d));
                let _ = SelectObject(dc, oldp);
                name_w = name_w.max(tw);
                pill_w = pill_w.max(pw + PILL_PAD * 2);
            }
            let _ = ReleaseDC(HWND::default(), dc);
            let rows = devices.len() as i32;
            (
                MARGIN_X + DOT + DOT_GAP + name_w + PANEL_GAP_X + pill_w + MARGIN_X * 2,
                rows.max(1) * PANEL_ROW_H + PANEL_PAD * 2,
            )
        }
    }

    fn paint_panel(&self, hwnd: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);

            let md = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
            let oldbmp = SelectObject(md, HGDIOBJ(bmp.0));
            let theme = theme::detect();

            // chroma fill, then a solid dark card on top
            let chroma = CreateSolidBrush(COLORREF(theme::CHROMA));
            let _ = FillRect(md, &rc, chroma);
            let _ = DeleteObject(HGDIOBJ(chroma.0));
            let card = RECT {
                left: 0,
                top: 0,
                right: rc.right,
                bottom: rc.bottom,
            };
            let card_br = CreateSolidBrush(COLORREF(rgb(0x20, 0x22, 0x28)));
            let _ = FillRect(md, &card, card_br);
            let _ = DeleteObject(HGDIOBJ(card_br.0));

            let low_alert = self.settings.lock().unwrap().low_battery_alert;
            let devices = self.panel_devices();
            for (i, d) in devices.iter().enumerate() {
                let y = PANEL_PAD + i as i32 * PANEL_ROW_H;
                let cy = y + PANEL_ROW_H / 2;
                let dcol = if !d.connected {
                    theme.dot_off
                } else if d.battery.map(|b| b <= low_alert).unwrap_or(false) {
                    theme.dot_low
                } else {
                    theme.dot_ok
                };
                let dbr = CreateSolidBrush(COLORREF(dcol));
                let drect = RECT {
                    left: MARGIN_X,
                    top: cy - DOT / 2,
                    right: MARGIN_X + DOT,
                    bottom: cy + DOT / 2 + 1,
                };
                let _ = FillRect(md, &drect, dbr);
                let _ = DeleteObject(HGDIOBJ(dbr.0));

                let old = SelectObject(md, HGDIOBJ(self.name_font().0));
                draw_text_halo(
                    md,
                    MARGIN_X + DOT + DOT_GAP,
                    cy - text_size(md, &d.name).1 / 2,
                    &d.name,
                    theme.fg,
                    theme.fg_outline,
                );
                let _ = SelectObject(md, old);

                let ptext = pill_text(d);
                let (pcolor, pfg) = match d.battery {
                    Some(v) if v <= low_alert => (theme.pill_low, rgb(255, 255, 255)),
                    Some(v) if v <= 50 => (theme.pill_mid, rgb(255, 255, 255)),
                    Some(_) => (theme.pill_ok, rgb(255, 255, 255)),
                    None => (theme.pill_off_bg, theme.pill_off_fg),
                };
                let old2 = SelectObject(md, HGDIOBJ(self.pill_font().0));
                let (pw, ph) = text_size(md, &ptext);
                let px = rc.right - MARGIN_X * 2 - pw - PILL_PAD * 2;
                let pbr = CreateSolidBrush(COLORREF(pcolor));
                let prect = RECT {
                    left: px,
                    top: cy - ph / 2 - 2,
                    right: px + pw + PILL_PAD * 2,
                    bottom: cy + ph / 2 + 2,
                };
                let _ = FillRect(md, &prect, pbr);
                let _ = DeleteObject(HGDIOBJ(pbr.0));
                draw_text_halo(
                    md,
                    px + PILL_PAD,
                    cy - ph / 2,
                    &ptext,
                    pfg,
                    theme.fg_outline,
                );
                let _ = SelectObject(md, old2);
            }

            let _ = BitBlt(
                hdc,
                0,
                0,
                rc.right,
                rc.bottom,
                md,
                0,
                0,
                ROP_CODE(0x00CC0020),
            );
            let _ = SelectObject(md, oldbmp);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(md);
            let _ = EndPaint(hwnd, &ps);
        }
    }

    unsafe fn handle_panel_message(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_PAINT => {
                self.paint_panel(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_LBUTTONUP | WM_RBUTTONUP | WM_KILLFOCUS => {
                self.hide_panel();
                LRESULT(0)
            }
            WM_DESTROY => {
                self.panel_hwnd.store(0, Ordering::Relaxed);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn drain_alerts(&self) {
        let mut alerts = self.pending_alerts.lock().unwrap();
        if alerts.is_empty() {
            return;
        }
        let list: Vec<String> = alerts.drain(..).collect();
        drop(alerts);
        for a in list {
            self.show_balloon("蓝牙设备电量低", &a, true);
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    // ================================================================== wndproc
    unsafe fn handle_message(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_TIMER => {
                if wparam.0 == DEVICE_TIMER {
                    unsafe {
                        let _ = KillTimer(hwnd, DEVICE_TIMER);
                    }
                    self.request_refresh();
                } else {
                    self.on_timer();
                }
                LRESULT(0)
            }
            WM_DEVICECHANGE => {
                unsafe {
                    let _ = SetTimer(hwnd, DEVICE_TIMER, 400, None);
                }
                LRESULT(1)
            }
            MSG_DATA_READY => {
                // No fullscreen/heal work here — those run once per STRIP_TIMER
                // tick in on_timer(); data-ready only repaints.
                self.process_dirty_or_reanchor();
                LRESULT(0)
            }
            WM_CLOSE => {
                // Explicit quit: mark it BEFORE DestroyWindow so WM_DESTROY
                // can distinguish it from an Explorer-caused destroy.
                self.shutting_down.store(true, Ordering::Release);
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_PAINT => {
                self.paint();
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            TRAY_MSG => {
                let code = (lparam.0 & 0xFFFF) as u32;
                match code {
                    0x0203 => {} // WM_LBUTTONDBLCLK
                    0x0201 => self.on_command(MID_TOGGLE),
                    0x0205 => self.show_menu(),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let x = (lparam.0 & 0xFFFF) as u16 as i32;
                let (lo, hi) = *self.overflow_x.lock().unwrap();
                if hi > lo && x >= lo && x < hi {
                    self.toggle_panel();
                } else {
                    self.hide_panel();
                    self.on_command(MID_REFRESH);
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                self.show_menu();
                LRESULT(0)
            }
            WM_COMMAND => {
                self.on_command((wparam.0 & 0xFFFF) as usize);
                LRESULT(0)
            }
            // Explorer recreates the taskbar (restart / crash / DPI change):
            // re-run the embed self-heal so we become a child of the NEW
            // Shell_TrayWnd.
            msg if {
                let id = self.wm_taskbar_created.load(Ordering::Relaxed);
                id != 0 && msg == id
            } =>
            {
                crate::dblog::log("taskbar: TaskbarCreated received -> re-embedding");
                self.heal_embedding();
                self.layout_and_resize();
                LRESULT(0)
            }
            // Monitor topology or system setting changed: force a full
            // re-embed check and relayout (taskbar may have moved/resized).
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                self.heal_embedding();
                self.layout_and_resize();
                LRESULT(0)
            }
            WM_DESTROY => {
                // Distinguish "user asked to quit" (WM_CLOSE / menu Exit set
                // shutting_down first) from "Explorer died and took our
                // embedded child with it". A freshly restarted Explorer may
                // already have created a NEW Shell_TrayWnd by the time this
                // runs, so probing for the taskbar here is unreliable — the
                // embedded + shutting-down flags are the source of truth.
                let shutting_down = self.shutting_down.load(Ordering::Acquire);
                let was_embedded = self.embedded.load(Ordering::Relaxed);
                if should_schedule_rebuild_after_destroy(shutting_down, was_embedded) {
                    crate::dblog::log(
                        "taskbar: strip destroyed unexpectedly (Explorer restart); scheduling recreate",
                    );
                    self.embedded.store(false, Ordering::Relaxed);
                    self.taskbar_hwnd.store(0, Ordering::Relaxed);
                    // Release the tray icon that pointed at the dead hwnd;
                    // it is re-registered on the new window after recreate.
                    unsafe {
                        let nid = self.notify_data_for(hwnd, HICON::default(), "");
                        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                    }
                    let slot = *self.tray_icon.lock().unwrap();
                    if slot != 0 {
                        unsafe {
                            let _ = DeleteObject(HGDIOBJ(slot as *mut core::ffi::c_void));
                        }
                    }
                    *self.tray_icon.lock().unwrap() = 0;
                    *self.overflow_x.lock().unwrap() = (0, 0);
                    // The overflow panel is an independent topmost popup:
                    // destroy it or it lingers on screen forever.
                    self.destroy_panel();
                    self.hwnd.store(0, Ordering::Relaxed);
                    // Raw-input sink (helper), poll thread and HID watcher
                    // are all independent of this window — keep them running.
                    self.schedule_recreate();
                    return LRESULT(0);
                }
                self.shutting_down.store(true, Ordering::Release);
                self.pending_recreate.store(false, Ordering::Release);
                self.teardown_resources();
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    /// Messages for the hidden helper window (raw-input sink + broadcast
    /// receiver). Runs on the UI thread. The helper is the only window that
    /// reliably receives TaskbarCreated / WM_DISPLAYCHANGE /
    /// WM_SETTINGCHANGE once the main strip is an embedded WS_CHILD.
    unsafe fn handle_helper_message(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            MSG_RECREATE_STRIP => {
                let _ = KillTimer(hwnd, RECREATE_TIMER);
                self.try_recreate_strip();
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == RECREATE_TIMER => {
                // Delayed recreate retry — fire once per timer, re-armed by
                // try_recreate_strip if the attempt fails again.
                let _ = KillTimer(hwnd, RECREATE_TIMER);
                self.try_recreate_strip();
                LRESULT(0)
            }
            msg if {
                let tbc = self.wm_taskbar_created.load(Ordering::Relaxed);
                (tbc != 0 && msg == tbc) || msg == WM_DISPLAYCHANGE || msg == WM_SETTINGCHANGE
            } =>
            {
                if self.hwnd.load(Ordering::Relaxed) == 0 {
                    // Strip not rebuilt yet and we were orphaned: kick the
                    // recreate pipeline (TaskbarCreated means Explorer is
                    // back up — a good moment to retry).
                    if self.pending_recreate.load(Ordering::Acquire) {
                        let _ = PostMessageW(hwnd, MSG_RECREATE_STRIP, WPARAM(0), LPARAM(0));
                    }
                } else {
                    crate::dblog::log("taskbar: broadcast via helper -> heal + relayout");
                    self.heal_embedding();
                    self.layout_and_resize();
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    fn on_raw_input(&self, lparam: LPARAM) {
        let path = raw_input_device_path(lparam);
        if path.is_empty() {
            return;
        }
        let n = RAW_INPUT_LOGS.fetch_add(1, Ordering::Relaxed);
        if n < 24 {
            crate::dblog::log(&format!("wm_input {path}"));
        }
        if !hid::hid_path_is_x87_24g(&path) {
            return;
        }
        if hid::note_x87_key_activity() {
            crate::dblog::log("x87 2.4g key -> connected");
            if let Some(w) = self.hid_watch.lock().unwrap().as_ref() {
                w.kick();
            }
        }
    }

    // ================================================================== self capture (diagnostics)
    #[allow(dead_code)]
    fn capture_self(&self, path: &std::path::Path) {
        unsafe {
            let hwnd = self.hwnd();
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let w = rc.right;
            let h = rc.bottom;
            if w <= 0 || h <= 0 {
                return;
            }
            let dc = GetDC(HWND::default());
            let md = CreateCompatibleDC(dc);
            let bmp = CreateCompatibleBitmap(dc, w, h);
            let old = SelectObject(md, HGDIOBJ(bmp.0));
            let _ = PrintWindow(hwnd, md, 1);

            let mut bi = BITMAPINFOHEADER::default();
            bi.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.biWidth = w;
            bi.biHeight = -h; // top-down
            bi.biPlanes = 1;
            bi.biBitCount = 32;
            bi.biCompression = BI_RGB.0;
            let mut bmi = BITMAPINFO {
                bmiHeader: bi,
                bmiColors: [std::mem::zeroed()],
            };
            let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
            GetDIBits(
                md,
                bmp,
                0,
                h as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            );

            let _ = SelectObject(md, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(md);
            let _ = ReleaseDC(HWND::default(), dc);

            write_bmp(path, w as u32, h as u32, &buf);
            crate::dblog::log(&format!(
                "capture_self saved {} ({}x{})",
                path.display(),
                w,
                h
            ));
        }
    }
}

// ---------------------------------------------------------------------------
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        if let Some(app) = APP.get() {
            app.on_raw_input(lparam);
        }
        return LRESULT(0);
    }
    if let Some(app) = APP.get() {
        let panel = app.panel_hwnd.load(Ordering::Relaxed);
        if panel != 0 && panel == hwnd.0 as i64 {
            return app.handle_panel_message(hwnd, msg, wparam, lparam);
        }
        if app.hwnd().0 == hwnd.0 {
            return app.handle_message(hwnd, msg, wparam, lparam);
        }
        // Hidden helper window: raw-input sink + broadcast receiver +
        // recreate scheduler.
        if app.raw_hwnd.load(Ordering::Relaxed) == hwnd.0 as i64 {
            return app.handle_helper_message(hwnd, msg, wparam, lparam);
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Popup panel procedure: click anywhere / lose focus -> close.
unsafe extern "system" fn panel_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(app) = APP.get()
        && app.panel_hwnd.load(Ordering::Relaxed) == hwnd.0 as i64
    {
        return app.handle_panel_message(hwnd, msg, wparam, lparam);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

// ---------------------------------------------------------------------------
fn poll_loop(app: Arc<App>, rx: Receiver<()>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    btreader::start_connection_watcher();
    loop {
        app.publish_bt(btreader::enumerate());
        let interval = {
            let connected = app.snapshot.lock().unwrap().has_connected;
            let cfg = app.settings.lock().unwrap();
            if connected {
                cfg.poll_interval()
            } else {
                Duration::from_secs(15)
            }
        };
        match rx.recv_timeout(interval) {
            Ok(()) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    unsafe {
        CoUninitialize();
    }
}

// ---------------------------------------------------------------------------
/// Hidden top-level helper window. Replaces the old HWND_MESSAGE raw sink:
/// message-only windows (and WS_CHILD windows like the embedded strip) never
/// receive broadcasts such as TaskbarCreated / WM_DISPLAYCHANGE /
/// WM_SETTINGCHANGE, so the helper is a real top-level WS_POPUP instead.
/// - never shown (no ShowWindow call anywhere)
/// - WS_EX_TOOLWINDOW: absent from taskbar and Alt-Tab
/// - WS_EX_NOACTIVATE + non-topmost: never steals focus or covers anything
/// It stays registered as the RIDEV_INPUTSINK target for the HID keyboard.
fn create_helper_window(hinst: HINSTANCE, class_name: PCWSTR) -> HWND {
    unsafe {
        let helper = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0),
            class_name,
            w!("BtBatteryBarHelper"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            hinst,
            None,
        )
        .unwrap_or_default();
        if !helper.0.is_null() {
            crate::dblog::log("helper window: hidden top-level popup (broadcasts + raw sink)");
        } else {
            crate::dblog::log("helper window: creation failed");
        }
        helper
    }
}

fn register_raw_keyboard(hwnd: HWND) {
    if hwnd.0.is_null() {
        crate::dblog::log("raw register skipped: null hwnd");
        return;
    }
    let rid = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x06,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    unsafe {
        match RegisterRawInputDevices(&[rid], std::mem::size_of::<RAWINPUTDEVICE>() as u32) {
            Ok(()) => crate::dblog::log("raw register ok (keyboard sink)"),
            Err(e) => crate::dblog::log(&format!("raw register failed: {e}")),
        }
    }
}

fn unregister_raw_keyboard() {
    let rid = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x06,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: HWND::default(),
    };
    unsafe {
        let _ = RegisterRawInputDevices(&[rid], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    }
}

fn raw_input_device_path(lparam: LPARAM) -> String {
    unsafe {
        let mut size = 0u32;
        let hdr = std::mem::size_of::<RAWINPUTHEADER>() as u32;
        let _ = GetRawInputData(HRAWINPUT(lparam.0 as _), RID_INPUT, None, &mut size, hdr);
        if size == 0 {
            return String::new();
        }
        let mut buf = vec![0u8; size as usize];
        let got = GetRawInputData(
            HRAWINPUT(lparam.0 as _),
            RID_INPUT,
            Some(buf.as_mut_ptr().cast()),
            &mut size,
            hdr,
        );
        if got == u32::MAX || got == 0 {
            return String::new();
        }
        let header = &*(buf.as_ptr() as *const RAWINPUTHEADER);
        let mut nchars = 0u32;
        let _ = GetRawInputDeviceInfoW(header.hDevice, RIDI_DEVICENAME, None, &mut nchars);
        if nchars == 0 {
            return String::new();
        }
        let mut name = vec![0u16; nchars as usize];
        let _ = GetRawInputDeviceInfoW(
            header.hDevice,
            RIDI_DEVICENAME,
            Some(name.as_mut_ptr().cast()),
            &mut nchars,
        );
        String::from_utf16_lossy(name.split(|c| *c == 0).next().unwrap_or(&name))
    }
}

/// Locate the taskbar (Shell_TrayWnd) and the *left edge* of the notification
/// tray cluster, all in SCREEN coordinates. Returns `None` when Explorer's
/// taskbar does not exist right now.
fn find_taskbar() -> Option<(HWND, RECT, Option<i32>)> {
    unsafe {
        let shell = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).unwrap_or_default();
        if shell.0.is_null() {
            return None;
        }
        let mut tb = RECT::default();
        let _ = GetWindowRect(shell, &mut tb);
        if tb.right <= tb.left || tb.bottom <= tb.top {
            return None;
        }
        Some((shell, tb, find_tray_left(shell)))
    }
}

fn find_tray_left(shell: HWND) -> Option<i32> {
    unsafe {
        let mut h = FindWindowExW(shell, HWND::default(), w!("TrayNotifyWnd"), PCWSTR::null())
            .unwrap_or_default();
        if h.0.is_null() {
            h = FindWindowExW(shell, HWND::default(), w!("SystemTray"), PCWSTR::null())
                .unwrap_or_default();
        }
        if h.0.is_null() {
            h = FindWindowW(w!("NotifyIconOverflowWindow"), PCWSTR::null()).unwrap_or_default();
        }
        if !h.0.is_null() {
            let mut r = RECT::default();
            let _ = GetWindowRect(h, &mut r);
            if r.right > r.left {
                return Some(r.left);
            }
        }
        None
    }
}

/// Where `hwnd`'s client-area origin sits in SCREEN coordinates.
unsafe fn client_origin_screen(hwnd: HWND) -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    // ScreenToClient maps screen->client; applied to (0,0) it yields minus
    // the client origin expressed in screen coords.
    let _ = ScreenToClient(hwnd, &mut p);
    POINT { x: -p.x, y: -p.y }
}

/// Translate a screen-space rect into a coordinate space whose origin is at
/// `origin_screen` (pure; unit-testable).
fn rect_offset_by(r: RECT, origin_screen: POINT) -> RECT {
    RECT {
        left: r.left - origin_screen.x,
        top: r.top - origin_screen.y,
        right: r.right - origin_screen.x,
        bottom: r.bottom - origin_screen.y,
    }
}

/// Taskbar geometry in the taskbar's OWN client coordinates — exactly what a
/// WS_CHILD strip needs for SetWindowPos. `None` when there is no taskbar.
fn taskbar_client_geometry() -> Option<(RECT, Option<i32>)> {
    let (_shell, tb, tray_left) = find_taskbar()?;
    unsafe {
        let origin = client_origin_screen(_shell);
        let tb_client = rect_offset_by(tb, origin);
        let tray_left_client = tray_left.map(|l| l - origin.x);
        Some((tb_client, tray_left_client))
    }
}

/// Pure layout math for the EMBEDDED strip: given the taskbar's own client
/// rect, the notification area's left edge in the same coordinates, the dock
/// side and the desired width, return `(x, y, w, h)` for SetWindowPos on a
/// WS_CHILD window. The strip docks next to the tray cluster, clamped to stay
/// inside the taskbar no matter how narrow it is (vertical taskbars etc.).
fn strip_placement_in_taskbar(
    tb_client: RECT,
    tray_left_client: Option<i32>,
    dock_left: bool,
    width: i32,
) -> (i32, i32, i32, i32) {
    let tb_w = (tb_client.right - tb_client.left).max(0);
    let tb_h = (tb_client.bottom - tb_client.top).max(MIN_STRIP_HEIGHT);
    let avail_w = (tb_w - 4).max(MIN_STRIP_WIDTH);
    let w = width.clamp(MIN_STRIP_WIDTH, avail_w);
    let right_edge = match tray_left_client {
        Some(l) if l > tb_client.left => l - GAP_TRAY,
        _ => tb_client.right - TASKBAR_RESERVE,
    };
    let max_x = (right_edge - w).max(tb_client.left + 2);
    let x = if dock_left {
        (tb_client.left + LEFT_INSET).min(max_x)
    } else {
        max_x
    };
    (x, tb_client.top, w, tb_h)
}

/// Pure layout math for the DEGRADED floating strip: overlay the taskbar in
/// plain SCREEN coordinates (the window stays an independent non-topmost
/// popup). Returns `(x, y, h)`; width is used as-is.
fn strip_placement_on_screen(
    tb_screen: RECT,
    tray_left_screen: Option<i32>,
    dock_left: bool,
    width: i32,
) -> (i32, i32, i32) {
    let tb_h = (tb_screen.bottom - tb_screen.top).max(MIN_STRIP_HEIGHT);
    let right_edge = match tray_left_screen {
        Some(l) if l > tb_screen.left => l - GAP_TRAY,
        _ => tb_screen.right - TASKBAR_RESERVE,
    };
    let max_x = (right_edge - width).max(tb_screen.left + 2);
    let x = if dock_left {
        (tb_screen.left + LEFT_INSET).min(max_x)
    } else {
        max_x
    };
    (x, tb_screen.top, tb_h)
}

/// True when the foreground window is a genuine fullscreen window — it covers
/// an entire monitor and isn't the shell. Used to auto-hide the floating strip
/// during fullscreen video / games so it never draws over the picture.
fn is_fullscreen_app_active(own_hwnd: HWND) -> bool {
    const FS_SLACK: i32 = 8; // tolerate 1px-off window rects
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == own_hwnd || IsIconic(fg).as_bool() {
            return false;
        }
        // Ignore the shell itself (desktop, taskbars).
        let mut cls = [0u16; 64];
        let n = GetClassNameW(fg, &mut cls).max(0) as usize;
        let cls = String::from_utf16_lossy(&cls[..n.min(cls.len())]);
        if matches!(
            cls.as_str(),
            "Progman"
                | "WorkerW"
                | "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
                | "XamlExplorerHostIslandWindow"
        ) {
            return false;
        }
        // Skip cloaked windows (invisible UWP hosts can be fullscreen-sized).
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(
            fg,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return false;
        }
        // Does the foreground window cover a whole monitor?
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() {
            return false;
        }
        let mr = mi.rcMonitor;
        let mut wr = RECT::default();
        let _ = GetWindowRect(fg, &mut wr);
        wr.left <= mr.left + FS_SLACK
            && wr.top <= mr.top + FS_SLACK
            && wr.right >= mr.right - FS_SLACK
            && wr.bottom >= mr.bottom - FS_SLACK
    }
}

// ---------------------------------------------------------------------------
fn create_font(height: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            1, // DEFAULT_CHARSET
            0, // OUT_DEFAULT_PRECIS
            0, // CLIP_DEFAULT_PRECIS
            3, // NONANTIALIASED_QUALITY — avoids magenta fringes with color-key
            0, // DEFAULT_PITCH
            w!("Segoe UI"),
        )
    }
}

fn draw_text_halo(dc: HDC, x: i32, y: i32, s: &str, fg: u32, outline: u32) {
    unsafe {
        let mut wcs: Vec<u16> = s.encode_utf16().collect();
        wcs.push(0);
        let _ = SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, COLORREF(outline));
        for (dx, dy) in [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ] {
            let _ = TextOutW(dc, x + dx, y + dy, &wcs);
        }
        let _ = SetTextColor(dc, COLORREF(fg));
        let _ = TextOutW(dc, x, y, &wcs);
    }
}

fn text_size(dc: HDC, s: &str) -> (i32, i32) {
    unsafe {
        let buf: Vec<u16> = s.encode_utf16().collect();
        let mut size = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &buf, &mut size);
        (size.cx, size.cy)
    }
}

fn pill_text(d: &crate::model::Device) -> String {
    match d.battery {
        Some(v) => format!("{v}%"),
        None => "--".to_string(),
    }
}

fn worst_battery(snap: &Snapshot) -> Option<u8> {
    snap.devices
        .iter()
        .filter(|d| d.battery.is_some())
        .map(|d| d.battery.unwrap())
        .min()
}

fn fill_wchars(buf: &mut [u16], s: &str, max: usize) {
    let cap = buf.len().min(max);
    for (i, c) in s.encode_utf16().take(cap.saturating_sub(1)).enumerate() {
        buf[i] = c;
    }
    if cap > 0 {
        buf[cap - 1] = 0;
    }
}

fn checked_flag(on: bool) -> MENU_ITEM_FLAGS {
    MENU_ITEM_FLAGS(MF_STRING.0 | if on { MF_CHECKED.0 } else { 0 })
}

fn append_item(menu: HMENU, id: usize, text: PCWSTR, checked: bool) {
    unsafe {
        let _ = AppendMenuW(menu, checked_flag(checked), id, text);
    }
}

fn append_popup(parent: HMENU, child: HMENU, text: PCWSTR) {
    unsafe {
        let _ = AppendMenuW(parent, MF_POPUP, child.0 as usize, text);
    }
}

fn append_flags(
    menu: HMENU,
    flags: MENU_ITEM_FLAGS,
    id: usize,
    text: &str,
    keep: &mut Vec<Vec<u16>>,
) {
    keep.push(text.encode_utf16().chain(std::iter::once(0)).collect());
    let p = PCWSTR(keep.last().unwrap().as_ptr());
    unsafe {
        let _ = AppendMenuW(menu, flags, id, p);
    }
}

fn open_ms_settings(uri: &str) {
    let w: Vec<u16> = uri.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = ShellExecuteW(
            HWND::default(),
            w!("open"),
            PCWSTR(w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

pub(crate) fn set_autostart(enabled: bool) {
    unsafe {
        let run = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, run, 0, KEY_SET_VALUE, &mut key).0 != 0 {
            return;
        }
        if enabled {
            let exe = std::env::current_exe().unwrap_or_default();
            let value = format!("\"{}\"", exe.to_string_lossy().replace('/', "\\"));
            let mut bytes: Vec<u8> = value
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(|u| u.to_le_bytes())
                .collect();
            let _ = RegSetValueExW(key, w!("BtBatteryBar"), 0, REG_SZ, Some(&mut bytes));
        } else {
            let _ = RegDeleteValueW(key, w!("BtBatteryBar"));
        }
        let _ = RegCloseKey(key);
    }
}

fn max_strip_width() -> i32 {
    unsafe {
        let mut wa = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut wa as *mut _ as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        ((wa.right - wa.left) * 70) / 100
    }
}

/// Write a top-down BGRA buffer as a BMP file (rows reversed for bottom-up layout).
fn write_bmp(path: &std::path::Path, w: u32, h: u32, bgra_top_down: &[u8]) {
    use std::io::Write;
    let row_stride = w as usize * 4;
    if bgra_top_down.len() < row_stride * h as usize {
        return;
    }
    let file_size = 54 + bgra_top_down.len();
    let mut data = Vec::with_capacity(file_size);
    data.extend_from_slice(b"BM");
    data.extend_from_slice(&(file_size as u32).to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER
    data.extend_from_slice(&40u32.to_le_bytes());
    data.extend_from_slice(&w.to_le_bytes());
    data.extend_from_slice(&h.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&32u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&(bgra_top_down.len() as u32).to_le_bytes());
    data.extend_from_slice(&0i32.to_le_bytes());
    data.extend_from_slice(&0i32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    for row in (0..h as usize).rev() {
        let start = row * row_stride;
        data.extend_from_slice(&bgra_top_down[start..start + row_stride]);
    }
    if let Ok(mut f) = std::fs::File::create(path) {
        let _ = f.write_all(&data);
    }
}

// ---------------------------------------------------------------------------
// Pure layout math tests — run without any real Explorer / taskbar.
#[cfg(test)]
mod layout_tests {
    use super::{
        GAP_TRAY, LEFT_INSET, MIN_STRIP_HEIGHT, MIN_STRIP_WIDTH, TASKBAR_RESERVE, rect_offset_by,
        strip_placement_in_taskbar, strip_placement_on_screen,
    };
    use windows::Win32::Foundation::{POINT, RECT};

    fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
        RECT {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    /// Bottom taskbar on a 3840x2160 screen at 150% DPI (taskbar height 72):
    /// screen rect vs its own client rect.
    #[test]
    fn screen_rect_maps_into_taskbar_client_coords() {
        // Taskbar spans the full width; client origin sits at (12, 2088).
        let origin = POINT { x: 12, y: 2088 };
        let screen = rect(-12, 2088, 3852, 2160);
        let c = rect_offset_by(screen, origin);
        assert_eq!(c, rect(-24, 0, 3840, 72));
        // Tray left edge converts the same way.
        assert_eq!(3600 - origin.x, 3588);
    }

    #[test]
    fn negative_origin_secondary_monitor_left_of_primary() {
        let origin = POINT { x: -1920, y: 0 };
        let screen = rect(-1920, -60, 0, 0);
        let c = rect_offset_by(screen, origin);
        assert_eq!(c, rect(0, -60, 1920, 0));
    }

    #[test]
    fn embedded_docks_right_next_to_tray() {
        let tb = rect(0, 0, 3840, 72);
        let (x, y, w, h) = strip_placement_in_taskbar(tb, Some(3600), false, 400);
        assert_eq!(y, 0);
        assert_eq!(h, 72);
        assert_eq!(w, 400);
        // right edge must end exactly GAP_TRAY left of the tray cluster
        assert_eq!(x + w + GAP_TRAY, 3600);
    }

    #[test]
    fn embedded_docks_left_with_inset() {
        let tb = rect(0, 0, 3840, 72);
        let (x, y, w, h) = strip_placement_in_taskbar(tb, Some(3600), true, 400);
        assert_eq!(x, LEFT_INSET);
        assert_eq!(y, 0);
        assert_eq!(w, 400);
        assert_eq!(h, 72);
    }

    #[test]
    fn embedded_falls_back_to_reserve_when_tray_not_found() {
        let tb = rect(0, 0, 3840, 72);
        let (_, _, w, _) = strip_placement_in_taskbar(tb, None, false, 400);
        let (xr, _, _, _) = strip_placement_in_taskbar(tb, None, false, w);
        assert_eq!(xr + w, tb.right - TASKBAR_RESERVE);
    }

    #[test]
    fn embedded_overwide_strip_stays_inside_taskbar() {
        let tb = rect(0, 0, 2000, 48);
        let tray_left = 1800;
        // Absurdly wide strip cannot fit before the tray; the math must
        // still keep it inside the taskbar bounds.
        let (x, _, w, h) = strip_placement_in_taskbar(tb, Some(tray_left), false, 5000);
        assert_eq!(h, 48);
        assert_eq!(w, tb.right - tb.left - 4);
        assert!(x >= tb.left + 2);
        assert!(x + w <= tb.right);
    }

    #[test]
    fn embedded_vertical_taskbar_clamps_width_and_height() {
        // Left-docked vertical taskbar: narrow but tall.
        let tb = rect(0, 0, 80, 1080);
        let (x, y, w, h) = strip_placement_in_taskbar(tb, Some(80), true, 600);
        assert_eq!(y, 0);
        assert_eq!(h, 1080);
        assert_eq!(w, 76); // 80 - 4
        assert!(x >= tb.left + 2);
        assert!(x + w <= tb.right);
    }

    #[test]
    fn embedded_degenerate_height_gets_floor() {
        let tb = rect(0, 0, 1000, 10);
        let (_, _, _, h) = strip_placement_in_taskbar(tb, Some(900), false, 300);
        assert_eq!(h, MIN_STRIP_HEIGHT);
    }

    #[test]
    fn floating_overlay_matches_screen_coords_of_taskbar() {
        let tb = rect(0, 2064, 3840, 2160);
        let (x, y, h) = strip_placement_on_screen(tb, Some(3600), false, 400);
        assert_eq!(y, 2064);
        assert_eq!(h, 2160 - 2064);
        assert_eq!(x + 400 + GAP_TRAY, 3600);
    }

    #[test]
    fn floating_dock_left_uses_inset() {
        let tb = rect(0, 2064, 3840, 2160);
        let (x, y, _) = strip_placement_on_screen(tb, Some(3600), true, 400);
        assert_eq!(x, LEFT_INSET);
        assert_eq!(y, 2064);
    }

    #[test]
    fn min_width_respected_on_tiny_taskbars() {
        let tb = rect(0, 0, 20, 1080);
        let (_, _, w, _) = strip_placement_in_taskbar(tb, None, false, 600);
        assert_eq!(w, MIN_STRIP_WIDTH);
    }
}

// ---------------------------------------------------------------------------
// Pure policy tests — embedding / destroy decisions without any real window.
#[cfg(test)]
mod policy_tests {
    use super::{StripVisibility, should_schedule_rebuild_after_destroy, strip_visibility};

    /// A user-hidden strip must never be asked to show — not by embedding,
    /// not by healing, not after an Explorer rebuild.
    #[test]
    fn hidden_preference_never_requests_show() {
        assert_eq!(strip_visibility(false, false), StripVisibility::Hidden);
        assert_eq!(strip_visibility(false, true), StripVisibility::Hidden);
    }

    /// A user-visible strip shows unless a fullscreen app suppresses it.
    #[test]
    fn visible_preference_shows_unless_fullscreen_hidden() {
        assert_eq!(strip_visibility(true, false), StripVisibility::Shown);
        assert_eq!(strip_visibility(true, true), StripVisibility::Hidden);
    }

    /// Root-cause regression: when Explorer dies it destroys our embedded
    /// child. A NEW Shell_TrayWnd may already exist by the time WM_DESTROY
    /// runs, so the decision must rely on the embedded + shutdown flags and
    /// still schedule a rebuild.
    #[test]
    fn embedded_strip_destroyed_by_explorer_schedules_rebuild() {
        assert!(should_schedule_rebuild_after_destroy(false, true));
    }

    /// An explicit quit (WM_CLOSE / menu Exit) must never schedule a rebuild,
    /// and a non-embedded strip keeps the old quit behavior.
    #[test]
    fn explicit_quit_never_schedules_rebuild() {
        assert!(!should_schedule_rebuild_after_destroy(true, true));
        assert!(!should_schedule_rebuild_after_destroy(true, false));
        assert!(!should_schedule_rebuild_after_destroy(false, false));
    }
}
