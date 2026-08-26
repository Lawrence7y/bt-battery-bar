//! Data model shared between the reader, the strip UI and the tray.

use std::collections::HashSet;

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Device {
    pub name: String,
    pub address: String,
    pub is_le: bool,
    pub connected: bool,
    /// Battery 0..=100 when the device exposes the GATT Battery Service.
    pub battery: Option<u8>,
    /// "ok" | "no-battery" | "offline"
    pub status: &'static str,
    /// "Mouse" / "Keyboard" / "Headset" / "Gamepad" / "Pen" / "Trackpad" / "Unknown"
    pub kind: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    #[allow(dead_code)]
    pub scanned_at_ms: u64,
    pub has_connected: bool,
}

#[allow(dead_code)]
impl Snapshot {
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }
    pub fn any_connected(&self) -> bool {
        self.devices.iter().any(|d| d.connected)
    }
    pub fn status_summary(&self) -> String {
        if self.devices.is_empty() {
            return "无设备".to_string();
        }
        self.devices
            .iter()
            .map(|d| match d.battery {
                Some(b) => format!("{} {}%", short_name(&d.name), b),
                None => format!("{} --", short_name(&d.name)),
            })
            .collect::<Vec<_>>()
            .join("  ")
    }
}

pub fn short_name(name: &str) -> String {
    let n = name.trim();
    const MAX: usize = 22;
    if n.chars().count() <= MAX {
        n.to_string()
    } else {
        let mut s: String = n.chars().take(MAX).collect();
        s.push('…');
        s
    }
}

fn name_key(name: &str) -> String {
    name.to_lowercase()
        .replace("键盘", "")
        .replace("鼠标", "")
        .replace("keyboard", "")
        .replace("mouse", "")
        .replace("2.4g", "")
        .replace("有线", "")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .collect()
}

fn device_rank(d: &Device) -> (u8, u8, u8) {
    (
        u8::from(d.connected),
        u8::from(d.battery.is_some()),
        u8::from(d.address.to_ascii_lowercase().starts_with("hid:")),
    )
}

/// Collapse USB + Bluetooth copies of the same peripheral into one row.
/// A name group holding two or more distinct Bluetooth MACs is two physical
/// devices that happen to share a name (e.g. two identical mice) — those rows
/// are kept separate instead of being folded into one.
pub fn dedup_by_name(devs: Vec<Device>) -> Vec<Device> {
    // 1) group by name key, remembering insertion order
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<Device>> =
        std::collections::HashMap::new();
    for d in devs {
        let mut key = name_key(&d.name);
        if key.is_empty() {
            key = d.address.to_ascii_lowercase();
        }
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(d);
    }

    let mut out: Vec<Device> = Vec::new();
    for key in order {
        let Some(members) = groups.remove(&key) else {
            continue;
        };
        let bt_macs: HashSet<String> = members
            .iter()
            .filter(|d| !d.address.to_ascii_lowercase().starts_with("hid:"))
            .map(|d| d.address.to_ascii_lowercase())
            .collect();
        if bt_macs.len() >= 2 {
            // Several same-name physical devices: keep every row.
            out.extend(members);
        } else {
            // One physical device (possibly with a hid/USB copy): keep the best.
            if let Some(best) = members
                .into_iter()
                .max_by(|a, b| device_rank(a).cmp(&device_rank(b)))
            {
                out.push(best);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(name: &str, addr: &str, connected: bool, battery: Option<u8>) -> Device {
        Device {
            name: name.into(),
            address: addr.into(),
            is_le: !addr.starts_with("hid:"),
            connected,
            battery,
            status: "ok",
            kind: "Keyboard",
        }
    }

    #[test]
    fn dedup_keeps_live_hid_over_bluetooth_copy() {
        let out = dedup_by_name(vec![
            d("前行者 X87", "AA:BB:CC:DD:EE:FF", false, None),
            d("前行者 X87", "hid:0C45:FEFE:Keyboard", true, Some(17)),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].address, "hid:0C45:FEFE:Keyboard");
        assert_eq!(out[0].battery, Some(17));
    }

    #[test]
    fn dedup_strips_24g_and_wired_suffix() {
        let out = dedup_by_name(vec![
            d("前行者 X87", "AA:BB:CC:DD:EE:FF", false, None),
            d("前行者 X87 2.4G", "hid:0C45:FEFE:Keyboard", true, Some(17)),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].address, "hid:0C45:FEFE:Keyboard");
        let out2 = dedup_by_name(vec![
            d("前行者 X87 有线", "hid:0C45:FEFE:Keyboard", true, None),
            d("前行者 X87 2.4G", "AA:BB:CC:DD:EE:FF", false, Some(10)),
        ]);
        assert_eq!(out2.len(), 1);
        assert_eq!(out2[0].address, "hid:0C45:FEFE:Keyboard");
    }
}
