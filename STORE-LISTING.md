# BtBatteryBar — Microsoft Store 一览文案 / Store Listing Content

> 语言：英语(美国) en-us（如需简体中文一览，中文版附后）

---

## Description（说明）

**BtBatteryBar — see every device battery right on your taskbar.**

BtBatteryBar is a lightweight Windows utility that embeds a mini battery bar directly into your taskbar, showing real-time battery levels of your connected Bluetooth and 2.4G devices — mice, keyboards, headsets, controllers and more.

⭐ **Highlights**
- **Truly embedded in the taskbar** — sits next to the clock as a native part of the taskbar (not a floating overlay), automatically follows taskbar position/size/DPI, and survives Explorer restarts.
- **Broad device support** — reads the standard Bluetooth GATT Battery Service (0x180F), Logitech HID++ (Unifying/Lightspeed/receivers/wired), plus common 2.4G dongle protocols (Compx/ATK/Sonix & more) via plain system APIs. No extra drivers needed.
- **Fast & event-driven** — connect/disconnect detected within ~1–2 seconds via system events; devices that support push notifications update instantly through GATT Notify.
- **Feather-light** — single ~0.4 MB native executable, no runtime, no Electron, no WebView; idles around 10–12 MB of RAM.
- **Color-coded at a glance** — green ≥50%, orange 20–49%, red <20%, with tray bubble alerts when a device drops to 20%.
- **Overflow panel** — too many devices? A "+n" pill opens the full list so nothing is ever cut off.
- **Full-screen friendly** — when embedded it never covers your games or videos; even in fallback floating mode it auto-hides during full-screen apps.

Perfect for anyone who has ever had a wireless mouse die mid-meeting. Pair your devices in Windows Bluetooth settings, run BtBatteryBar, and never guess a battery level again.

---

## What's new in this version（此版本的新增功能）

Initial release on Microsoft Store.

- Taskbar-embedded battery bar for Bluetooth & 2.4G devices
- Tray icon and menu: polling interval, autostart, low-battery alerts
- Logitech HID++, ATK/Compx/Sonix 2.4G protocol support
- Explorer-restart self-healing and DPI awareness

---

## Product features（产品功能，每条 ≤100 字符左右）

1. Real-time battery levels for Bluetooth & 2.4G devices on your taskbar
2. Truly embedded next to the clock — not a floating overlay
3. Supports GATT Battery Service, Logitech HID++ and common 2.4G protocols
4. Event-driven updates: devices appear within seconds of connecting
5. Color-coded battery pills: green / orange / red at a glance
6. Low-battery tray notifications (customizable threshold)
7. Adjustable polling interval: 30 s / 1 min / 5 min
8. Overflow "+n" panel when many devices are connected
9. Survives Explorer restarts with automatic re-embedding
10. Extremely lightweight: ~0.4 MB binary, ~10 MB RAM, no dependencies
11. Autostart with Windows
12. Dark & light theme aware
13. Demo mode to preview the UI (`bt-battery-bar.exe demo`)
14. Debug CLI: `probe` / `hidprobe` JSON output for enthusiasts

---

## Short title（短标题）

BtBatteryBar

## Short description（简短描述）

See battery levels of your Bluetooth & 2.4G mouse, keyboard and headset right on the Windows taskbar.

## Keywords（关键字，最多 7 个）

bluetooth, battery, taskbar, mouse, keyboard, headset, 2.4G

## Copyright and trademark info（版权和商标信息）

Copyright © 2025 Lawrence7YY. All rights reserved.

---

# 简体中文（zh-CN）一览

## 说明

**BtBatteryBar —— 在任务栏上直接查看所有设备电量。**

BtBatteryBar 是一款轻量级 Windows 工具，它在任务栏内部嵌入一条迷你电量条，实时显示已连接的蓝牙和 2.4G 设备（鼠标、键盘、耳机、手柄等）的剩余电量。

⭐ **特色**
- **真正嵌入任务栏**：紧挨时钟显示，是任务栏的一部分（非悬浮窗），自动跟随任务栏位置/尺寸/DPI，Explorer 重启后自动恢复。
- **广泛设备支持**：标准蓝牙 GATT 电池服务、罗技 HID++（Unifying/Lightspeed/接收器/有线）、常见 2.4G 接收器协议（Compx/ATK/Sonix 等），纯系统 API，无需驱动。
- **事件驱动、刷新快**：设备连接/断开约 1–2 秒内显示；支持推送的设备电量即时更新。
- **极致轻量**：单文件约 0.4 MB 原生程序，无运行时依赖，空闲内存约 10–12 MB。
- **颜色一目了然**：绿 ≥50%、橙 20–49%、红 <20%；电量 ≤20% 时托盘气泡提醒。
- **溢出面板**：设备过多时显示 "+n"，点击查看完整列表。
- **全屏友好**：内嵌模式绝不遮挡游戏/视频；降级浮动模式下检测到全屏自动隐藏。

## 此版本的新增功能

首次上架 Microsoft Store。

## 产品功能

1. 任务栏实时显示蓝牙与 2.4G 设备电量
2. 真正内嵌时钟旁，非悬浮覆盖
3. 支持 GATT 电池服务、罗技 HID++ 与常见 2.4G 协议
4. 事件驱动：设备连接数秒内出现
5. 电量胶囊分色：绿/橙/红一目了然
6. 低电量托盘提醒（阈值可调）
7. 轮询间隔可调：30 秒 / 1 分钟 / 5 分钟
8. 多设备时显示 "+n" 溢出面板
9. Explorer 重启自动恢复嵌入
10. 极致轻量：约 0.4 MB、约 10 MB 内存、零依赖
11. 支持开机自启
12. 自动跟随系统深色/浅色主题
13. 内置 demo 模式预览界面
14. 提供 probe/hidprobe 调试命令输出 JSON

## 关键字

蓝牙, 电量, 任务栏, 鼠标, 键盘, 耳机, 无线

---

# 图片对照表（对应 Partner Center 上传位）

| Partner Center 位置 | 文件 | 尺寸 |
|---|---|---|
| 屏幕截图 | `msix/store/screenshot-1.png` | 1920×1080 |
| 屏幕截图（特写） | `msix/store/screenshot-2.png` | 1920×1080 |
| 9:16 招贴画 | `msix/store/poster-9x16.png` | 720×1080 |
| 1:1 酷图 | `msix/store/boxart-1x1.png` | 1080×1080 |
| 16:9 主角图像 | `msix/store/hero-16x9.png` | 1920×1080（不含产品名）|
| 1:1 应用磁贴图标 | `msix/store/tile-icon-300x300.png` | 300×300 |
| 1:1 | `msix/store/tile-150x150.png` | 150×150 |
| 1:1 | `msix/store/tile-71x71.png` | 71×71 |

截图为真实程序渲染（demo 数据）：先运行 `release\bt-battery-bar.exe demo`，
再运行 `python tools\make_store_assets.py` 即可重新生成全部图片。
