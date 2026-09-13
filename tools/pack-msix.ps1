# Pack the MSIX: build -> verify -> copy payload -> sync manifest version -> makeappx -> optional sign
#
# NOTE: this file is intentionally ASCII-only. Windows PowerShell 5.1 parses .ps1
# files using the ANSI code page unless they carry a UTF-8 BOM, so non-ASCII
# comments can break parsing depending on the editor/tooling that touched the file.
# Rationale and usage notes live in msix/README.md (UTF-8, not executed).
#
# Why a script instead of a manual copy:
#   1. A hand-copied msix/layout/bt-battery-bar.exe drifts from the source. That
#      shipped a console-subsystem exe to the Store: double-clicking opened a
#      terminal window covering the desktop.
#   2. A hand-copied exe can miss +crt-static from .cargo/config.toml and then
#      fails to start on machines without the VC++ runtime.
# Both checks below are hard failures (fail fast) before anything is packaged:
#   - PE Subsystem == 2 (WINDOWS_GUI)
#   - no VCRUNTIME140 / MSVCP140 dependency (static CRT)
#
# Usage:
#   pwsh tools/pack-msix.ps1                        # version from Cargo.toml, unsigned
#   pwsh tools/pack-msix.ps1 -AppxVersion 0.1.2.0   # explicit 4-part package version
#   pwsh tools/pack-msix.ps1 -Sign -Pfx msix\BtBatteryBar_SelfSigned.pfx
param(
    [string]$AppxVersion = "",
    [switch]$Sign,
    [string]$Pfx = "",
    [string]$PfxPassword = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$script:RepoRoot = Split-Path -Parent $PSScriptRoot
$root = $script:RepoRoot
. (Join-Path $PSScriptRoot "verify-payload.ps1")
$layout = Join-Path $root "msix\layout"
$manifest = Join-Path $layout "AppxManifest.xml"
$exe = Join-Path $root "target\release\bt-battery-bar.exe"

function Get-SdkTool([string]$name) {
    $c = Get-Command $name -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $pattern = "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\$name"
    $candidates = Get-ChildItem -Path $pattern -ErrorAction SilentlyContinue | Sort-Object FullName -Descending
    if ($candidates) { return $candidates[0].FullName }
    throw "Cannot find $name (needs the Windows SDK)"
}

# ---- 1. version ----
if (-not $AppxVersion) {
    $AppxVersion = Get-CrateVersion -FourParts
}
if ($AppxVersion -notmatch '^\d+\.\d+\.\d+\.\d+$') { throw "AppxVersion must be X.Y.Z.W (got '$AppxVersion')" }
Write-Host "package version: $AppxVersion"

# ---- 2. build ----
if (-not $SkipBuild) {
    Write-Host "==> cargo build --release"
    Push-Location $root
    try { Invoke-Native "cargo build failed" { cargo build --release } }
    finally { Pop-Location }
}
if (-not (Test-Path $exe)) { throw "missing $exe" }

# ---- 3. hard verification: GUI subsystem + static CRT ----
Write-Host "==> verifying payload"
Assert-Payload -Exe $exe

# ---- 4. copy payload + sync manifest version ----
Copy-Item $exe (Join-Path $layout "bt-battery-bar.exe") -Force
Write-Host "==> copied payload to msix\layout\bt-battery-bar.exe"

# Read/write the manifest as explicit UTF-8 *without* BOM: PowerShell 5.1's
# Get-Content/Set-Content default to the ANSI code page for BOM-less files and
# would double-encode the Chinese Description attribute into invalid XML.
$utf8 = New-Object System.Text.UTF8Encoding($false)
$xml = [System.IO.File]::ReadAllText($manifest, $utf8)
$updated = [regex]::Replace($xml, '(<Identity\b[^>]*?Version=")[^"]+(")', ('${1}' + $AppxVersion + '${2}'))
if ($updated -ne $xml) {
    [System.IO.File]::WriteAllText($manifest, $updated, $utf8)
    Write-Host "==> AppxManifest.xml version synced to $AppxVersion"
} else {
    Write-Host "==> AppxManifest.xml version unchanged ($AppxVersion)"
}

# ---- 5. pack ----
$out = Join-Path $root "msix\BtBatteryBar-$AppxVersion.msix"
$makeappx = Get-SdkTool "makeappx.exe"
Write-Host "==> makeappx pack -> $out"
Invoke-Native "makeappx failed" -OnlyMatching "Packing |succeeded|error|Error" { & $makeappx pack /d $layout /p $out /o }
Write-Host ("    artifact: {0} ({1:N0} KB)" -f $out, ((Get-Item $out).Length / 1KB))

# ---- 6. optional signing ----
if ($Sign) {
    if (-not $Pfx) { throw "-Sign requires -Pfx <file.pfx>" }
    $signtool = Get-SdkTool "signtool.exe"
    Write-Host "==> signtool sign"
    Invoke-Native "signing failed" { & $signtool sign /fd SHA256 /f $Pfx /p $PfxPassword $out }
}
Write-Host "done."
