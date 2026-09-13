//! Logitech HID++ (VID 0x046D) battery queries over vendor HID collections.
//!
//! HID++ 2.0 frames ride in short (0x10, 7-byte) or long (0x11, 20-byte)
//! reports: [report_id, device_index, feature_index, function<<4, params...].
//! The Root feature (index 0x00, function 0) maps well-known battery feature
//! IDs to the per-device feature index; then we read the status:
//!   0x1002 Unified Battery       f1 GetStatus            -> byte0 = charge %
//!   0x1000 Battery Status        f1 GetBatteryStatus     -> byte0 = level
//!   0x1001 Battery Level Status  f0 GetBatteryLevelStatus-> byte0 = level
//!
//! Wireless devices behind a Unifying/Lightspeed receiver answer on device
//! index 1..=6; USB-connected devices answer on 0xFF. We try 0xFF first, then
//! 0x01, and remember devices that never answer so polls stay cheap.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;

use crate::hid::{note_live_battery, overlapped_read, overlapped_write};

const SHORT: u8 = 0x10;
const LONG: u8 = 0x11;
const RESP_TIMEOUT_MS: u32 = 350;
const RESP_MAX_READS: usize = 3;

/// Devices that never answered a HID++ query — skip them for a while.
static HIDPP_DEAD: LazyLock<Mutex<HashMap<(u16, u16), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
const DEAD_TTL: Duration = Duration::from_secs(10 * 60);

/// Build one HID++ request frame. Total length matches the short/long report
/// size so WriteFile accepts it as a complete output report.
pub fn build_req(short: bool, dev: u8, feat: u8, func: u8, params: &[u8]) -> Vec<u8> {
    let total = if short { 7 } else { 20 };
    let mut v = vec![0u8; total];
    v[0] = if short { SHORT } else { LONG };
    v[1] = dev;
    v[2] = feat;
    v[3] = func << 4;
    for (i, p) in params.iter().enumerate() {
        let idx = 4 + i;
        if idx < total {
            v[idx] = *p;
        }
    }
    v
}

/// Extract the payload (bytes 4..) of a matching HID++ response frame.
pub fn parse_resp<'a>(buf: &'a [u8], dev: u8, feat: u8, func: u8) -> Option<&'a [u8]> {
    if buf.len() < 5 {
        return None;
    }
    let rid = buf[0];
    if rid != SHORT && rid != LONG {
        return None;
    }
    if buf[1] != dev || buf[2] != feat || (buf[3] >> 4) != func {
        return None;
    }
    Some(&buf[4..])
}

/// 该集合应发短帧还是长帧？None = 集合放不下 HID++ 输出报文。
///
/// 关键：报告长度必须与集合的 OutputReportByteLength 一致。
/// Unifying 接收器的 HID++ 集合是 7 字节输出；Lightspeed / 有线设备常见 20 字节，
/// 必须用长帧，否则 WriteFile 直接失败。
fn frame_is_short(out_len: u16) -> Option<bool> {
    if out_len < 7 {
        None
    } else {
        Some(out_len < 20)
    }
}

/// Send one request and wait for the matching response payload.
fn transact(
    handle: HANDLE,
    input_len: u16,
    out_len: u16,
    dev: u8,
    feat: u8,
    func: u8,
    params: &[u8],
) -> Option<Vec<u8>> {
    unsafe {
        // HID++ 帧长度必须与集合的 OutputReportByteLength 一致，否则 WriteFile 会以
        // ERROR_INVALID_PARAMETER 失败（见 build_req 文档）：
        //   7..20 字节的集合 -> 7 字节短帧（report id 0x10）
        //   >= 20 字节的集合 -> 20 字节长帧（report id 0x11）
        // 曾经写成 `short = out_len >= 7`，于是 20 字节的 Lightspeed 集合被塞 7 字节
        // 短帧，这类设备的 HID++ 电量永远查不到。
        let short = frame_is_short(out_len)?;
        let req = build_req(short, dev, feat, func, params);
        overlapped_write(handle, &req).ok()?;

        let deadline = Instant::now() + Duration::from_millis(RESP_TIMEOUT_MS as u64 * 2);
        for _ in 0..RESP_MAX_READS {
            if Instant::now() >= deadline {
                break;
            }
            let buf = overlapped_read(handle, input_len.max(8) as usize, RESP_TIMEOUT_MS).ok()?;
            if let Some(payload) = parse_resp(&buf, dev, feat, func) {
                return Some(payload.to_vec());
            }
            // Unrelated notification (media keys, link events...) — keep reading.
        }
        None
    }
}

/// Root.GetFeatureId: map a well-known feature id to the device-local index.
fn feature_index(
    handle: HANDLE,
    input_len: u16,
    out_len: u16,
    dev: u8,
    feature_id: u16,
) -> Option<u8> {
    let params = [(feature_id >> 8) as u8, (feature_id & 0xFF) as u8];
    let resp = transact(handle, input_len, out_len, dev, 0x00, 0x00, &params)?;
    let idx = *resp.first()?;
    if idx == 0 {
        return None; // feature not supported
    }
    Some(idx)
}

fn battery_via(
    handle: HANDLE,
    input_len: u16,
    out_len: u16,
    dev: u8,
    feature_id: u16,
    func: u8,
) -> Option<u8> {
    let idx = feature_index(handle, input_len, out_len, dev, feature_id)?;
    let resp = transact(handle, input_len, out_len, dev, idx, func, &[])?;
    let pct = *resp.first()?;
    if pct <= 100 { Some(pct) } else { None }
}

fn battery_for_device(handle: HANDLE, input_len: u16, out_len: u16, dev: u8) -> Option<u8> {
    // Unified Battery reports an explicit percentage — prefer it.
    if let Some(pct) = battery_via(handle, input_len, out_len, dev, 0x1002, 0x01) {
        return Some(pct);
    }
    if let Some(pct) = battery_via(handle, input_len, out_len, dev, 0x1000, 0x01) {
        return Some(pct);
    }
    if let Some(pct) = battery_via(handle, input_len, out_len, dev, 0x1001, 0x00) {
        return Some(pct);
    }
    None
}

/// Query a Logitech HID++ collection for its battery percentage.
pub fn query_battery(
    handle: HANDLE,
    input_len: u16,
    out_len: u16,
    vid: u16,
    pid: u16,
) -> Option<u8> {
    if vid != 0x046D {
        return None;
    }
    {
        let dead = HIDPP_DEAD.lock().ok()?;
        if let Some(at) = dead.get(&(pid, 0))
            && at.elapsed() < DEAD_TTL
        {
            return None;
        }
    }
    for dev in [0xFFu8, 0x01] {
        if let Some(pct) = battery_for_device(handle, input_len, out_len, dev) {
            note_live_battery(vid, pid, pct);
            if let Ok(mut dead) = HIDPP_DEAD.lock() {
                dead.remove(&(pid, 0));
            }
            return Some(pct);
        }
    }
    if let Ok(mut dead) = HIDPP_DEAD.lock() {
        dead.insert((pid, 0), Instant::now());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_request_frame_layout() {
        let req = build_req(true, 0xFF, 0x00, 0x00, &[0x10, 0x02]);
        assert_eq!(req.len(), 7);
        assert_eq!(req[0], 0x10);
        assert_eq!(req[1], 0xFF);
        assert_eq!(req[2], 0x00);
        assert_eq!(req[3], 0x00);
        assert_eq!(&req[4..6], &[0x10, 0x02]);
    }

    #[test]
    fn long_request_frame_layout() {
        let req = build_req(false, 0x01, 0x07, 0x01, &[]);
        assert_eq!(req.len(), 20);
        assert_eq!(req[0], 0x11);
        assert_eq!(req[3], 0x10); // func 1 << 4
    }

    #[test]
    fn parse_resp_matches_frame_identity() {
        let buf = [0x10u8, 0xFF, 0x07, 0x10, 0x55, 0x00, 0x00];
        let resp = parse_resp(&buf, 0xFF, 0x07, 0x01).unwrap();
        assert_eq!(resp[0], 0x55);
        assert!(parse_resp(&buf, 0x01, 0x07, 0x01).is_none());
        assert!(parse_resp(&buf, 0xFF, 0x07, 0x00).is_none());
        assert!(parse_resp(&[0x01, 2, 3, 4, 5], 0xFF, 0x07, 0x01).is_none());
    }

    #[test]
    fn parse_resp_rejects_short_frames() {
        assert!(parse_resp(&[0x10, 0xFF, 0x00], 0xFF, 0x00, 0x00).is_none());
    }

    #[test]
    fn frame_length_matches_collection_report_size() {
        // Unifying 接收器（7 字节输出）走短帧
        assert_eq!(frame_is_short(7), Some(true));
        // 放不下输出报文的集合
        assert_eq!(frame_is_short(0), None);
        assert_eq!(frame_is_short(6), None);
        // Lightspeed / 有线设备的 20 字节集合必须用长帧（回归：旧代码发 7 字节短帧）
        assert_eq!(frame_is_short(20), Some(false));
        assert_eq!(frame_is_short(33), Some(false));
        let long = build_req(frame_is_short(20).unwrap(), 0xFF, 0x00, 0x00, &[0x10, 0x02]);
        assert_eq!(long.len(), 20);
        assert_eq!(long[0], 0x11);
    }
}
