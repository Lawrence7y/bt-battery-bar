# bt-battery-bar 图形安装包

一键双击、**无命令行窗口（无控制台黑框）**、无需管理员权限的 Windows 安装程序。
单文件 `BtBatteryBar-Setup.exe` 内嵌了主程序与卸载程序，双击即可完成安装。

## 文件说明

| 文件 | 说明 |
|---|---|
| `BtBatteryBar-Setup.exe` | 自包含图形安装器（内嵌主程序 + 卸载程序），双击安装 |
| `BtBatteryBar-Uninstall.exe` | 图形卸载程序（安装后会复制到安装目录，一般无需手动运行） |
| `build.bat` | 构建脚本：重新编译上面的两个 exe（需 .NET Framework 4.x，无需联网） |
| `Setup.cs` / `Uninstall.cs` | 安装器 / 卸载器源码 |
| `assets/icon.ico` | 安装器图标 |

## 安装

1. 双击 `BtBatteryBar-Setup.exe`，弹出图形界面（不会出现任何命令行黑框）。
2. 默认安装到 `%LOCALAPPDATA%\BtBatteryBar`（可修改），可勾选：
   - 开机自动启动（HKCU Run）
   - 安装完成后立即运行
3. 点击「安装」即可。安装后可从 **设置 → 应用** 中卸载（已注册标准卸载项），
   也可直接运行安装目录里的 `uninstall.exe`。

### 静默安装 / 卸载（可选）

- 静默安装：`BtBatteryBar-Setup.exe /S`（安装到默认目录，开启自启并启动程序）
- 静默卸载：`BtBatteryBar-Uninstall.exe /S`

## 特性

- **纯 GUI，无控制台窗口**：安装器和卸载器都编译为 Windows GUI 子系统（子系统 = 2），
  双击时不会闪烁任何命令行窗口。
- **无需管理员 / 无 UAC**：安装到当前用户目录（`%LOCALAPPDATA%`），自启写 HKCU Run，
  完全按用户级安装，不污染系统目录。
- **自包含单文件**：主程序 `bt-battery-bar.exe` 与卸载程序都嵌入在 setup 内，
  拷贝安装包即可分发，无需附带其它文件。
- **干净卸载**：卸载会停止进程、删除文件、移除开机自启与注册表卸载项，并自动删除自身。

## 构建

```bat
rem 在 installer 目录下执行（或任意目录执行 installer\build.bat）
build.bat
```

前提：Windows 10/11 x64，已安装 .NET Framework 4.x（系统自带）与 Rust（用于先构建主程序）。
脚本使用 `%WINDIR%\Microsoft.NET\Framework64\v4.0.30319\csc.exe` 编译，无需联网下载任何东西。

主程序需先由 `cargo build --release` 构建并放到 `dist\bt-battery-bar.exe`，
再运行 `build.bat` 生成安装包。
