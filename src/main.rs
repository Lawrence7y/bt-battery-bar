//! bt-battery-bar — 轻量 Windows 任务栏内嵌蓝牙/2.4G 设备电量条（Rust / Win32 / WinRT）。
//!
//! Usage:
//!   bt-battery-bar                        启动（任务栏内嵌条 + 托盘）
//!   bt-battery-bar probe                  枚举一次蓝牙设备并打印 JSON（调试）
//!   bt-battery-bar hidprobe               枚举 HID(2.4G 接收器) 并尝试读电量（调试）
//!   bt-battery-bar hidspy                 对 2.4G 厂商接口做读写探测（调试）
//!   bt-battery-bar hidlisten [秒]         持续监听 2.4G 厂商报告（调试）
//!   bt-battery-bar battprops              检查 Windows 是否暴露设备电量属性（调试）

mod app;
mod btreader;
mod dblog;
mod hid;
mod hidpp;
mod hidwatch;
mod icon;
mod model;
mod settings;
mod theme;

fn main() {
    // 单实例保护：重复启动（开机自启 + 手动双击）直接退出。
    // 内核互斥体句柄不关闭也无需保活：进程存活期间对象一直存在，
    // 进程退出时由系统统一回收。
    unsafe {
        use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows::Win32::System::Threading::CreateMutexW;
        use windows::core::PCWSTR;
        let name: Vec<u16> = "BtBatteryBarSingleInstance\0".encode_utf16().collect();
        if CreateMutexW(None, false, PCWSTR(name.as_ptr())).is_ok()
            && GetLastError() == ERROR_ALREADY_EXISTS
        {
            return;
        }
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|a| a == "probe" || a == "--probe" || a == "-p")
    {
        let devs = btreader::enumerate();
        println!("{}", serde_json::to_string_pretty(&devs).unwrap());
        return;
    }

    if args.iter().any(|a| a == "battprops" || a == "--battprops") {
        dump_battery_props();
        return;
    }

    if args.iter().any(|a| a == "hidlisten" || a == "--hidlisten") {
        let secs = args
            .iter()
            .find_map(|a| a.parse::<u64>().ok())
            .unwrap_or(40);
        hidwatch::listen(secs);
        return;
    }

    if args.iter().any(|a| a == "hidscan" || a == "--hidscan") {
        hid::scan();
        return;
    }

    if args.iter().any(|a| a == "scan2" || a == "--scan2") {
        hid::scan2();
        return;
    }

    if args.iter().any(|a| a == "feat" || a == "--feat") {
        hid::feat_probe();
        return;
    }

    if args.iter().any(|a| a == "hidspy" || a == "--hidspy") {
        hid::spy();
        return;
    }

    if args.iter().any(|a| a == "via" || a == "--via") {
        hid::via_probe();
        return;
    }

    if args.iter().any(|a| a == "hidprobe" || a == "--hidprobe") {
        let (devs, fails) = hid::enumerate();
        println!("=== HID collections ===");
        for d in &devs {
            println!(
                "{:04X}:{:04X} uP={:04X} u={:04X} in={} out={} feat={} acc={} prod=|{}| mfg=|{}| battery={:?} link={} bytes=[{}] dbg=[{}] path={}",
                d.vid,
                d.pid,
                d.usage_page,
                d.usage,
                d.input_len,
                d.output_len,
                d.feature_len,
                d.access,
                d.product,
                d.manufacturer,
                d.battery,
                d.link_up,
                d.feature_hex,
                d.debug,
                d.path
            );
        }
        println!("=== UI devices ===");
        for d in hid::enumerate_devices() {
            println!(
                "{} connected={} battery={:?} status={} kind={} addr={}",
                d.name, d.connected, d.battery, d.status, d.kind, d.address
            );
        }
        let hid_fails: Vec<&String> = fails
            .iter()
            .filter(|f| {
                let p = f.to_lowercase();
                p.contains("vid_373b") || p.contains("vid_046d")
            })
            .collect();
        if !hid_fails.is_empty() {
            println!("--- 373B/046D 相关接口（被拒/无法打开） ---");
            for f2 in hid_fails {
                println!("  {f2}");
            }
        } else if !fails.is_empty() {
            println!(
                "--- 其他接口无法打开（系统独占，可忽略）: {} 个 ---",
                fails.len()
            );
        }
        return;
    }

    let app = app::App::new();
    if args.iter().any(|a| a == "demo" || a == "--demo") {
        app.enable_demo_data();
    }
    let _ = app::APP.set(app.clone());
    let code = app.run();
    std::process::exit(code);
}

fn dump_battery_props() {
    println!("=== Bluetooth GATT ===");
    let bt = btreader::enumerate();
    println!("{}", serde_json::to_string_pretty(&bt).unwrap());
    println!("=== HID / 2.4G ===");
    let hid = hid::enumerate_devices();
    println!("{}", serde_json::to_string_pretty(&hid).unwrap());
}
