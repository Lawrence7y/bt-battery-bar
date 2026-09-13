# 隐私政策 / Privacy Policy

最后更新 / Last updated: 2025-08-23

本隐私政策适用于应用程序 **BtBatteryBar（bt-battery-bar）**。

This Privacy Policy applies to the application **BtBatteryBar (bt-battery-bar)**.

---

## 中文

### 概述

BtBatteryBar 是一款运行在本地的 Windows 桌面工具，用于在任务栏显示已连接的蓝牙 / 2.4G 设备电量。我们高度重视你的隐私。**本程序不收集、不上传、不共享任何个人数据。**

### 我们收集哪些数据

**无。** 本程序：

- 不收集任何个人信息、设备标识符或使用统计；
- 不包含任何遥测、分析或广告 SDK；
- 不连接任何远程服务器，没有任何网络通信；
- 不读取、存储或传输你的文件、联系人、位置、浏览记录等任何用户数据。

### 程序访问了什么

程序仅通过 Windows 系统本地 API（WinRT GATT 电池服务、HID 接口）读取你**已配对蓝牙 / 2.4G 设备的电量百分比与设备名称**，这些信息：

- 仅用于在屏幕上显示电量条；
- 仅保存在本机：用户设置写入 `%LOCALAPPDATA%\BtBatteryBar\settings.json`，开机自启项写入注册表 `HKCU\...\Run`，另有一份本地排障日志 `%LOCALAPPDATA%\BtBatteryBar\debug.log`（含设备名/地址与电量，仅本机、上限 1 MiB 自动轮转），**绝不出本机**；
- 可随时通过卸载程序完全清除。

### 权限说明

| 权限 | 用途 |
|---|---|
| 蓝牙（GATT） | 读取已配对设备的电池服务电量 |
| HID | 读取 2.4G 接收器 / 有线设备的电量报告 |

程序不会尝试配对新设备、不会更改设备设置、不会发送任何数据到设备之外。

### 联系方式

如对本政策有任何疑问，请通过 GitHub Issues 联系：
https://github.com/Lawrence7y/bt-battery-bar/issues

### 政策变更

如有变更将在本页面发布并更新"最后更新"日期。

---

## English

### Overview

BtBatteryBar is a local Windows desktop utility that displays battery levels of connected Bluetooth / 2.4G devices on the taskbar. We take your privacy seriously. **This application does not collect, transmit, or share any personal data.**

### What data we collect

**None.** This application:

- Does not collect any personal information, device identifiers, or usage statistics;
- Contains no telemetry, analytics, or advertising SDKs;
- Does not connect to any remote server and performs no network communication;
- Does not read, store, or transmit your files, contacts, location, or browsing history.

### What the app accesses

The app reads only the **battery percentage and device name of your already-paired Bluetooth / 2.4G devices** through local Windows system APIs (WinRT GATT Battery Service, HID interfaces). This information:

- Is used solely to display the battery bar on your screen;
- Is stored only locally: settings in `%LOCALAPPDATA%\BtBatteryBar\settings.json`, the autostart entry under `HKCU\...\Run`, plus a local troubleshooting log at `%LOCALAPPDATA%\BtBatteryBar\debug.log` (device names/addresses and battery levels; local only, capped at 1 MiB with rotation) — and **never leaves your machine**;
- Can be fully removed at any time by uninstalling the application.

### Permissions

| Permission | Purpose |
|---|---|
| Bluetooth (GATT) | Read battery level from paired devices' Battery Service |
| HID | Read battery reports from 2.4G receivers / wired devices |

The app never attempts to pair new devices, never changes device settings, and never sends any data off the device.

### Contact

If you have any questions about this policy, please open a GitHub issue:
https://github.com/Lawrence7y/bt-battery-bar/issues

### Changes to this policy

Any changes will be posted on this page with an updated "Last updated" date.
