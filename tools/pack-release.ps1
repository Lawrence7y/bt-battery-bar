# Pack the GitHub release artifacts:
#   build -> verify payload -> dist/ + release/ -> rebuild the GUI installers -> zip
#
# Same reasoning as tools/pack-msix.ps1: hand-copied binaries drift from the source
# (that is how a console-subsystem exe reached the Store package). The payload is
# verified before anything is copied or embedded.
#
# ASCII-only on purpose (PowerShell 5.1 parses .ps1 with the ANSI code page unless
# the file has a UTF-8 BOM). Notes in Chinese live in installer/README.md.
#
# Usage:
#   pwsh tools/pack-release.ps1
#   pwsh tools/pack-release.ps1 -SkipBuild
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$script:RepoRoot = Split-Path -Parent $PSScriptRoot
$root = $script:RepoRoot
. (Join-Path $PSScriptRoot "verify-payload.ps1")

$version = Get-CrateVersion
Write-Host "release version: $version"

# ---- 1. build ----
$exe = Join-Path $root "target\release\bt-battery-bar.exe"
if (-not $SkipBuild) {
    Write-Host "==> cargo build --release"
    Push-Location $root
    try { Invoke-Native "cargo build failed" { cargo build --release } }
    finally { Pop-Location }
}

# ---- 2. verify before shipping anything ----
Write-Host "==> verifying payload"
Assert-Payload -Exe $exe

# ---- 3. distribute the payload ----
$dist = Join-Path $root "dist"
$release = Join-Path $root "release"
foreach ($dir in @($dist, $release)) {
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
}
Copy-Item $exe (Join-Path $dist "bt-battery-bar.exe") -Force
Copy-Item $exe (Join-Path $release "bt-battery-bar.exe") -Force
# Keep the shipped README identical to the repo README instead of a stale copy.
Copy-Item (Join-Path $root "README.md") (Join-Path $dist "README.md") -Force
Write-Host "==> dist\bt-battery-bar.exe, release\bt-battery-bar.exe, dist\README.md synced"

# ---- 4. rebuild the GUI installers (they embed dist\bt-battery-bar.exe) ----
$buildBat = Join-Path $root "installer\build.bat"
if (Test-Path $buildBat) {
    Write-Host "==> installer\build.bat"
    Invoke-Native "installer build failed" { & cmd /c "`"$buildBat`"" }

    $setup = Join-Path $root "installer\BtBatteryBar-Setup.exe"
    $uninst = Join-Path $root "installer\BtBatteryBar-Uninstall.exe"

    # The setup embeds the app as a binary resource: check the freshly built exe's
    # header really is inside it, otherwise the installer would ship an old payload.
    $head = [System.IO.File]::ReadAllBytes($exe)[0..1023]
    $setupBytes = [System.IO.File]::ReadAllBytes($setup)
    $found = $false
    for ($i = 0; $i -le ($setupBytes.Length - $head.Length); $i++) {
        if ($setupBytes[$i] -eq $head[0]) {
            $ok = $true
            for ($j = 1; $j -lt $head.Length; $j++) {
                if ($setupBytes[$i + $j] -ne $head[$j]) { $ok = $false; break }
            }
            if ($ok) { $found = $true; break }
        }
    }
    if (-not $found) { throw "BtBatteryBar-Setup.exe does not embed the freshly built exe (stale installer?)" }
    Write-Host "    verified: setup embeds the current payload"

    Copy-Item $setup (Join-Path $dist "BtBatteryBar-Setup.exe") -Force
    Copy-Item $uninst (Join-Path $dist "BtBatteryBar-Uninstall.exe") -Force
    Write-Host "==> installers copied to dist\"
} else {
    Write-Host "!! installer\build.bat not found - skipping installers"
}

# ---- 5. zip the release artifacts ----
Add-Type -AssemblyName System.IO.Compression.FileSystem
function New-Zip([string]$ZipPath, [string[]]$Files) {
    if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
    $zip = [System.IO.Compression.ZipFile]::Open($ZipPath, 'Create')
    try {
        foreach ($f in $Files) {
            $leaf = Split-Path $f -Leaf
            [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $f, $leaf)
            Write-Host "    + $leaf"
        }
    } finally { $zip.Dispose() }
}

Write-Host "==> release\bt-battery-bar-$version-setup.zip (portable)"
New-Zip (Join-Path $release "bt-battery-bar-$version-setup.zip") @(
    (Join-Path $dist "bt-battery-bar.exe"),
    (Join-Path $dist "install.bat"),
    (Join-Path $dist "install.ps1"),
    (Join-Path $dist "uninstall.bat"),
    (Join-Path $dist "uninstall.ps1"),
    (Join-Path $dist "README.md")
)

Write-Host "==> release\bt-battery-bar-$version-installer.zip (GUI installer)"
New-Zip (Join-Path $release "bt-battery-bar-$version-installer.zip") @(
    (Join-Path $root "installer\BtBatteryBar-Setup.exe"),
    (Join-Path $root "installer\BtBatteryBar-Uninstall.exe"),
    (Join-Path $root "installer\README.md")
)

Get-ChildItem $release -Filter "*.zip" | ForEach-Object {
    Write-Host ("    artifact: {0} ({1:N0} KB)" -f $_.Name, ($_.Length / 1KB))
}
Write-Host "done."
