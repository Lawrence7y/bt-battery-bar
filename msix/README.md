# MSIX 打包说明 / MSIX Packaging

## 包信息（Microsoft Store Partner Center 分配）

| 项 | 值 |
|---|---|
| Package/Identity/Name | `Lawrence7YY.BtBatteryBar` |
| Package/Identity/Publisher | `CN=B2250643-15B9-4016-82B3-C97EAFA5DABD` |
| PublisherDisplayName | `Lawrence7YY` |
| Package Family Name | `Lawrence7YY.BtBatteryBar_9nqdfk2wdejvt` |
| Store ID | `9P6547H7G37P` |
| 版本 | 0.1.0.0 |

## 目录结构

```
msix/
├── layout/                    # 打包布局（makeappx 输入）
│   ├── AppxManifest.xml       # 包清单（含商店分配的标识）
│   ├── bt-battery-bar.exe     # 主程序（从 release/ 复制）
│   └── Assets/                # 应用图标资源（44/150/StoreLogo + targetsize 系列）
└── BtBatteryBar-0.1.0.0.msix  # 签名后的安装包
```

> ⚠️ `*.pfx` 签名证书与 `*.msix` 成品不入库（见 `.gitignore`）。

## 重新打包步骤

```powershell
# 1. 更新 layout 中的 exe
copy release\bt-battery-bar.exe msix\layout\

# 2. 打包（Windows SDK 的 makeappx）
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64\makeappx.exe" `
    pack /d msix\layout /p msix\BtBatteryBar-0.1.0.0.msix /o

# 3. 签名（自签名证书，CN 必须与清单 Publisher 一致）
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.19041.0\x64\signtool.exe" `
    sign /fd SHA256 /f msix\BtBatteryBar_SelfSigned.pfx /p <密码> msix\BtBatteryBar-0.1.0.0.msix
```

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
Add-AppxPackage -Path .\BtBatteryBar-0.1.0.0.msix
```

或直接双击 `.msix` 文件按提示安装。

## 提交到 Microsoft Store

在 [Partner Center](https://partner.microsoft.com/dashboard) 提交时上传
`BtBatteryBar-0.1.0.0.msix` 即可——清单中的 Identity 已与商店分配的
PFN / Store ID (`9P6547H7G37P`) 一致，微软会用商店证书重新签名。
