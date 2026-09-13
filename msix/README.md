# MSIX 打包说明 / MSIX Packaging

## 包信息（Microsoft Store Partner Center 分配）

| 项 | 值 |
|---|---|
| Package/Identity/Name | `Lawrence7YY.BtBatteryBar` |
| Package/Identity/Publisher | `CN=B2250643-15B9-4016-82B3-C97EAFA5DABD` |
| PublisherDisplayName | `Lawrence7YY` |
| Package Family Name | `Lawrence7YY.BtBatteryBar_9nqdfk2wdejvt` |
| Store ID | `9P6547H7G37P` |
| 版本 | 0.1.3.0 |

## 目录结构

```
msix/
├── layout/                    # 打包布局（makeappx 输入）
│   ├── AppxManifest.xml       # 包清单（含商店分配的标识）
│   ├── bt-battery-bar.exe     # 主程序（从 release/ 复制）
│   └── Assets/                # MSIX 图标、主题变体、倍率与磁贴资源
└── BtBatteryBar-0.1.3.0.msix  # 签名后的安装包
```

> ⚠️ `*.pfx` 签名证书与 `*.msix` 成品不入库（见 `.gitignore`）。

## 重新打包步骤（一条命令）

```powershell
pwsh tools\pack-msix.ps1                    # 版本取自 Cargo.toml，打包但不签名
pwsh tools\pack-msix.ps1 -Sign -Pfx msix\BtBatteryBar_SelfSigned.pfx -PfxPassword <密码>
```

脚本会依次完成：`cargo build --release` → **校验产物** → 复制到 `layout\` → 同步清单版本 →
`makeappx pack` →（可选）`signtool sign`。

### 打包前的两个硬校验（失败即中止）

| 校验 | 为什么 |
|---|---|
| `PE Subsystem == 2 (WINDOWS_GUI)` | 曾经把 **console 子系统**的 exe 打进商店包：用户双击会弹出一个终端黑窗盖住桌面（实机复现过）。`src/main.rs` 的 `#![windows_subsystem = "windows"]` 是这条校验的依据 |
| 不依赖 `VCRUNTIME140.dll` / `MSVCP140.dll` | 说明 `.cargo/config.toml` 的 `+crt-static` 没生效，产物在没装 VC++ 运行库的机器上起不来 |

因此 **`msix/layout/*.exe` 不再入库**（`.gitignore` 已忽略），只由脚本从 `target/release` 复制，
杜绝"包里的二进制落后于源码"。清单版本也由脚本写入，避免手工维护三处版本号（Cargo.toml / 清单 / 安装器）。

> ⚠️ 商店包版本必须**递增**：已发布 0.1.1.0，因此修复版打 0.1.2.0（`Cargo.toml` 已同步为 0.1.2）。
> 需要复现旧包时用 `-AppxVersion 0.1.1.0`，但不要用它覆盖已上架的版本号。

生成自签名证书（仅本地测试用；商店提交时由微软重新签名）：

```powershell
New-SelfSignedCertificate -Type Custom `
  -Subject "CN=B2250643-15B9-4016-82B3-C97EAFA5DABD" `
  -KeyUsage DigitalSignature -FriendlyName "BtBatteryBar MSIX" `
  -CertStoreLocation "Cert:\CurrentUser\My" `
  -NotAfter (Get-Date).AddYears(3) `
  -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3","2.5.29.19={text}")
```

## 本地安装测试

```powershell
# 1. 将证书导入"受信任的人"存储（管理员 PowerShell）
Import-Certificate -FilePath .\BtBatteryBar_SelfSigned.cer -CertStoreLocation Cert:\LocalMachine\TrustedPeople

# 2. 安装包
Add-AppxPackage -Path .\BtBatteryBar-0.1.3.0.msix
```

或直接双击 `.msix` 文件按提示安装。

## 提交到 Microsoft Store

在 [Partner Center](https://partner.microsoft.com/dashboard) 提交时上传
`BtBatteryBar-0.1.3.0.msix` 即可——清单中的 Identity 已与商店分配的
PFN / Store ID (`9P6547H7G37P`) 一致，微软会用商店证书重新签名。
