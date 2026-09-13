//! Persistent settings stored as JSON under %LOCALAPPDATA%\BtBatteryBar.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Device;

/// 记住的设备上限（超出后淘汰最旧的）。
const MAX_KNOWN_DEVICES: usize = 40;

/// 条内文字基准百分比（100 = 96 DPI 下的设计值）。
pub const DEFAULT_TEXT_PCT: u32 = 100;
/// 允许的字号范围：太小看不清，太大条会挤爆任务栏。
pub const MIN_TEXT_PCT: u32 = 70;
pub const MAX_TEXT_PCT: u32 = 160;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct KnownDevice {
    pub address: String,
    pub name: String,
    pub kind: String,
}

impl Default for KnownDevice {
    fn default() -> Self {
        Self {
            address: String::new(),
            name: String::new(),
            kind: "Unknown".into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub poll_seconds: u64,
    pub strip_visible: bool,
    /// Legacy field kept only so old settings.json files still parse. The
    /// strip is now embedded into the taskbar (WS_CHILD) and must never be
    /// globally topmost; the value is ignored and normalized to false.
    pub topmost: bool,
    pub autostart: bool,
    pub notify_low: bool,
    pub low_battery_alert: u8,
    pub resend_minutes: u64,
    pub opacity: u8,
    /// Show paired/offline devices on the strip (battery `--`).
    pub show_offline: bool,
    /// Addresses the user chose not to show on the strip.
    pub hidden_addresses: Vec<String>,
    /// Devices seen before, so the settings list stays stable when they sleep.
    pub known_devices: Vec<KnownDevice>,
    /// Taskbar dock: "left" or "right" (default).
    pub strip_side: String,
    /// 条内文字/间距的缩放百分比（相对 96 DPI 设计值），默认 100。
    /// 用户抱怨过 150% 缩放下字太大，因此给出 80/100/120/140 四档（菜单"字号"）。
    pub text_pct: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_seconds: 10,
            strip_visible: true,
            topmost: false,
            autostart: false,
            notify_low: true,
            low_battery_alert: 20,
            resend_minutes: 30,
            opacity: 100,
            show_offline: true,
            hidden_addresses: Vec::new(),
            known_devices: Vec::new(),
            strip_side: "right".into(),
            text_pct: DEFAULT_TEXT_PCT,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        let dir = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        dir.join("BtBatteryBar").join("settings.json")
    }

    pub fn load() -> Self {
        let p = Self::path();
        let mut cfg = match std::fs::read_to_string(&p) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        // TopMost was removed: the strip embeds into the taskbar and must
        // never float above fullscreen apps. Force the legacy field off.
        cfg.topmost = false;
        cfg
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        // 原子写：先写同目录临时文件，再 rename 覆盖。
        // 直接 fs::write 到目标文件时，崩溃/断电会留下半截 JSON，而 load() 解析失败
        // 会静默回退默认值 —— 用户设置会被无声清空，所以这里必须避免半截文件。
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            // Windows 上 std::fs::rename 使用 MOVEFILE_REPLACE_EXISTING，可覆盖已存在的目标
            if std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.poll_seconds.clamp(10, 3600))
    }

    /// 字号百分比，钳制到合法范围（settings.json 是用户可手改的）。
    pub fn text_pct(&self) -> u32 {
        self.text_pct.clamp(MIN_TEXT_PCT, MAX_TEXT_PCT)
    }

    pub fn dock_left(&self) -> bool {
        self.strip_side.eq_ignore_ascii_case("left")
    }

    pub fn is_shown(&self, address: &str) -> bool {
        !self
            .hidden_addresses
            .iter()
            .any(|a| a.eq_ignore_ascii_case(address))
    }

    pub fn set_shown(&mut self, address: &str, shown: bool) {
        let addr = address.to_string();
        self.hidden_addresses
            .retain(|a| !a.eq_ignore_ascii_case(&addr));
        if !shown {
            self.hidden_addresses.push(addr);
        }
    }

    /// Update the remembered catalog. Returns true if the file should be saved.
    pub fn remember(&mut self, devices: &[Device]) -> bool {
        let mut changed = false;
        for d in devices {
            if let Some(k) = self
                .known_devices
                .iter_mut()
                .find(|k| k.address.eq_ignore_ascii_case(&d.address))
            {
                if k.name != d.name || k.kind != d.kind {
                    k.name = d.name.clone();
                    k.kind = d.kind.to_string();
                    changed = true;
                }
            } else {
                self.known_devices.push(KnownDevice {
                    address: d.address.clone(),
                    name: d.name.clone(),
                    kind: d.kind.to_string(),
                });
                changed = true;
            }
        }
        if self.known_devices.len() > MAX_KNOWN_DEVICES {
            // 新设备追加在尾部：保留最新的一批，淘汰最旧的。
            // （旧实现用 truncate(40) 保留**前** 40 条，设备满了以后刚加进来的新设备会
            //  当场被截掉，永远记不住，而且每轮 remember() 都返回 true 反复写盘。）
            let drop = self.known_devices.len() - MAX_KNOWN_DEVICES;
            self.known_devices.drain(..drop);
            changed = true;
        }
        changed
    }

    pub fn filter_for_strip(&self, devices: &[Device]) -> Vec<Device> {
        devices
            .iter()
            .filter(|d| self.is_shown(&d.address) && (d.connected || self.show_offline))
            .cloned()
            .collect()
    }

    pub fn rows_for_settings(&self, live: &[Device]) -> Vec<DeviceRow> {
        let mut rows: Vec<DeviceRow> = live
            .iter()
            .map(|d| DeviceRow {
                address: d.address.clone(),
                name: d.name.clone(),
                kind: d.kind.to_string(),
                connected: d.connected,
                battery: d.battery,
                shown: self.is_shown(&d.address),
            })
            .collect();
        for k in &self.known_devices {
            if k.address.to_ascii_uppercase().contains("0C45:8006") {
                continue;
            }
            if rows
                .iter()
                .any(|r| r.address.eq_ignore_ascii_case(&k.address))
            {
                continue;
            }
            rows.push(DeviceRow {
                address: k.address.clone(),
                name: k.name.clone(),
                kind: k.kind.clone(),
                connected: false,
                battery: None,
                shown: self.is_shown(&k.address),
            });
        }
        rows.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        rows
    }
}

#[derive(Clone, Debug)]
pub struct DeviceRow {
    pub address: String,
    pub name: String,
    #[allow(dead_code)]
    pub kind: String,
    pub connected: bool,
    pub battery: Option<u8>,
    pub shown: bool,
}

impl DeviceRow {
    pub fn label(&self) -> String {
        let batt = match self.battery {
            Some(v) => format!("{v}%"),
            None => "--".into(),
        };
        let st = if self.connected {
            "已连接"
        } else {
            "离线"
        };
        format!("{}    {}  {}", self.name, batt, st)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, addr: &str, connected: bool, battery: Option<u8>) -> Device {
        Device {
            name: name.into(),
            address: addr.into(),
            is_le: false,
            connected,
            battery,
            status: if connected { "ok" } else { "offline" },
            kind: "Mouse",
        }
    }

    #[test]
    fn hidden_address_is_not_shown() {
        let mut s = Settings::default();
        assert!(s.is_shown("hid:373B:101B:Mouse"));
        s.set_shown("hid:373B:101B:Mouse", false);
        assert!(!s.is_shown("hid:373B:101B:Mouse"));
        assert!(!s.is_shown("HID:373B:101B:Mouse"));
        s.set_shown("hid:373B:101B:Mouse", true);
        assert!(s.is_shown("hid:373B:101B:Mouse"));
        assert!(s.hidden_addresses.is_empty());
    }

    #[test]
    fn filter_respects_hidden_and_offline() {
        let live = vec![
            dev("Mouse", "hid:m", true, Some(50)),
            dev("Headset", "bt:h", false, None),
        ];
        let mut s = Settings::default();
        let all = s.filter_for_strip(&live);
        assert_eq!(all.len(), 2);

        s.show_offline = false;
        let online = s.filter_for_strip(&live);
        assert_eq!(online.len(), 1);
        assert_eq!(online[0].name, "Mouse");

        s.show_offline = true;
        s.set_shown("hid:m", false);
        let rest = s.filter_for_strip(&live);
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].name, "Headset");
    }

    #[test]
    fn remember_merges_and_settings_rows_keep_known() {
        let mut s = Settings::default();
        assert!(s.remember(&[dev("Mouse", "hid:m", true, Some(10))]));
        assert!(!s.remember(&[dev("Mouse", "hid:m", true, Some(10))]));
        let rows = s.rows_for_settings(&[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Mouse");
        assert!(!rows[0].connected);
    }

    /// 回归：列表满 40 后，新设备必须仍然记得住，且不会每轮都触发保存。
    #[test]
    fn remember_keeps_newest_device_when_full() {
        let mut s = Settings::default();
        for i in 0..MAX_KNOWN_DEVICES {
            assert!(s.remember(&[dev(&format!("D{i}"), &format!("addr{i}"), true, None)]));
        }
        assert_eq!(s.known_devices.len(), MAX_KNOWN_DEVICES);
        // 第 41 个设备：应被记住（淘汰最旧的 D0），且只在这一轮返回 changed
        assert!(s.remember(&[dev("Newest", "addr-new", true, None)]));
        assert_eq!(s.known_devices.len(), MAX_KNOWN_DEVICES);
        assert!(
            s.known_devices.iter().any(|k| k.address == "addr-new"),
            "新设备必须被记住"
        );
        assert!(
            !s.known_devices.iter().any(|k| k.address == "addr0"),
            "应淘汰最旧的"
        );
        // 再记一次不应产生变化（旧实现会一直返回 true 反复写盘）
        assert!(!s.remember(&[dev("Newest", "addr-new", true, None)]));
    }

    #[test]
    fn settings_hides_legacy_wired_x87_address() {
        let mut s = Settings::default();
        s.known_devices.push(KnownDevice {
            address: "hid:0C45:8006:Keyboard".into(),
            name: "前行者 X87".into(),
            kind: "Keyboard".into(),
        });
        let live = [dev("前行者 X87", "hid:0C45:FEFE:Keyboard", true, Some(17))];
        let rows = s.rows_for_settings(&live);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].address, "hid:0C45:FEFE:Keyboard");
    }

    #[test]
    fn serde_roundtrip_hidden_list() {
        let mut s = Settings::default();
        s.set_shown("hid:x", false);
        s.show_offline = false;
        s.poll_seconds = 30;
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(!back.is_shown("hid:x"));
        assert!(!back.show_offline);
        assert_eq!(back.poll_seconds, 30);
    }

    #[test]
    fn text_pct_defaults_and_clamps() {
        let mut s = Settings::default();
        assert_eq!(s.text_pct(), DEFAULT_TEXT_PCT);
        // 手改 settings.json 也不能越界
        s.text_pct = 5;
        assert_eq!(s.text_pct(), MIN_TEXT_PCT);
        s.text_pct = 9999;
        assert_eq!(s.text_pct(), MAX_TEXT_PCT);
        // 缺字段的旧设置文件仍能解析，并落到默认档
        let legacy: Settings = serde_json::from_str(r#"{"poll_seconds":30}"#).unwrap();
        assert_eq!(legacy.text_pct(), DEFAULT_TEXT_PCT);
        // 往返
        s.text_pct = 120;
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.text_pct(), 120);
    }

    #[test]
    fn strip_side_defaults_right_and_roundtrips() {
        let s = Settings::default();
        assert!(!s.dock_left());
        let mut left = Settings::default();
        left.strip_side = "left".into();
        assert!(left.dock_left());
        let json = serde_json::to_string(&left).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(back.dock_left());
        let old = r#"{"poll_seconds":60}"#;
        let migrated: Settings = serde_json::from_str(old).unwrap();
        assert!(!migrated.dock_left());
    }

    #[test]
    fn topmost_defaults_false_and_legacy_true_is_not_reactivated() {
        assert!(!Settings::default().topmost);
        // Old JSON with topmost:true still parses, but the flag stays off.
        let legacy = r#"{"poll_seconds":30,"topmost":true}"#;
        let migrated: Settings = serde_json::from_str(legacy).unwrap();
        assert_eq!(migrated.poll_seconds, 30);
        // load() normalizes the legacy value to false; simulate that here.
        let normalized = {
            let mut c = migrated;
            c.topmost = false;
            c
        };
        assert!(!normalized.topmost);
    }
}
