# 发布清单 / Release checklist

一次发版 = **构建校验 → 提交打 tag → 推 GitHub → 建 Release → 提交商店**。下面每一步都可复制执行。

> 前置（只需一次）：本机需有 GitHub 凭据（`git push` 首次会走凭据管理器）与 `gh auth login`。
> 这两步涉及你的账号，脚本不会也不能代做。商店提交需要 Partner Center 登录（浏览器）。

---

## 1. 构建 + 校验（本地，无网络）

```powershell
pwsh tools\pack-release.ps1     # 便携版/安装器 zip + dist/ + release/
pwsh tools\pack-msix.ps1        # 商店包 msix\BtBatteryBar-<版本>.msix
```

两个脚本都会先做**硬校验**，任一不满足立即中止：

| 校验 | 拦截的问题 |
|---|---|
| `PE Subsystem == 2 (WINDOWS_GUI)` | 会把 console 子系统的 exe 打进包 → 用户双击弹出终端黑窗（0.1.1.0 曾发生） |
| 不依赖 `VCRUNTIME140.dll` / `MSVCP140.dll` | `.cargo/config.toml` 的 `+crt-static` 未生效 → 没装 VC++ 运行库的机器起不来 |
| `BtBatteryBar-Setup.exe` 内嵌**当前**主程序 | 安装器里塞的是旧 exe（手工 copy 的典型后果） |

版本号来自 `Cargo.toml`（清单会自动同步成 4 段）。**商店版本必须递增**。

## 2. 提交 + 打 tag

```powershell
git add -A
git commit -m "Release 0.1.3: ..."      # 或用上一条准备信息
git tag -a v0.1.3 -m "BtBatteryBar 0.1.3"
```

检查一下要提交的内容里没有构建产物（`.gitignore` 已忽略 `target/`、`release/*.zip`、`msix/*.msix`、`msix/layout/*.exe`）：

```powershell
git status --short
```

## 3. 推送到 GitHub + 建 Release（一条命令）

```powershell
pwsh tools\publish-release.ps1            # 推 main + tag，并创建 Release（附件：两个 zip）
pwsh tools\publish-release.ps1 -Draft     # 想先出草稿
```

Release 说明取自 `docs\release-notes\v<版本>.md`（UTF-8，可自由编辑）。

## 4. 提交 Microsoft Store

> 只能由你操作：需要 Partner Center 账号（浏览器登录）。命令行工具（`msstore` CLI）未安装。

1. 打开 <https://partner.microsoft.com/dashboard>，进入 **BtBatteryBar** 应用。
2. **Packages → 上传新包**：选择 `msix\BtBatteryBar-0.1.3.0.msix`。
   - 商店会用你自己的证书重新签名，本地自签名只用于本地安装测试；
   - 版本必须大于在架的 `0.1.1.0`（这就是本次打 `0.1.3.0` 的原因）。
3. 如提示，更新 **Store listing**（文案与素材见 `STORE-LISTING.md`，图片在 `msix/store/`）。
4. 提交审核 → 通过后商店用户自动更新。

可选但推荐：提交前用 **WACK** 自查（`C:\Program Files (x86)\Windows Kits\10\App Certification Kit\appcert.exe`），
需先把自签名证书导入"受信任的人"再安装包——详见 `msix\README.md`。

## 5. 发布后自检

```powershell
# 商店页（审核通过后）
start https://apps.microsoft.com/detail/9P6547H7G37P
# Release 页
gh release view v0.1.3 --web
```

---

## 常见问题

- **`git push` 报 could not read Username**：本机没有 GitHub 凭据。在终端里手动执行一次
  `git push origin main`，按提示登录（浏览器/令牌），之后再跑脚本即可。
- **`gh` 报 not logged into any GitHub hosts**：执行 `gh auth login`，选 GitHub.com → HTTPS → 浏览器登录。
- **`api.github.com` 返回 403 rate limit**：匿名 API 配额（60 次/小时）用尽，登录后为 5000 次/小时。
- **打包脚本报 payload is a console app**：检查 `src/main.rs` 顶部是否有 `#![windows_subsystem = "windows"]`。
- **打包脚本报 payload depends on VCRUNTIME140.dll**：`.cargo/config.toml` 丢了或未生效（应包含
  `rustflags = ["-C", "target-feature=+crt-static"]`）。
