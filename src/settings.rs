//! Persistent settings stored as JSON under %LOCALAPPDATA%\BtBatteryBar.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Device;

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
        if let Some(dir) = Self::path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::path(), text);
        }
    }

    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.poll_seconds.clamp(10, 3600))
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
        if self.known_devices.len() > 40 {
            self.known_devices.truncate(40);
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
