//! Minimal file logger for debugging (writes to %LOCALAPPDATA%\BtBatteryBar\debug.log).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    let dir = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("BtBatteryBar").join("debug.log")
}

pub fn log(msg: &str) {
    let p = path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = match OpenOptions::new().create(true).append(true).open(p) {
        Ok(f) => f,
        Err(_) => return,
    };
    let _ = writeln!(f, "{} {}", chronoish(), msg);
}

fn chronoish() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}
