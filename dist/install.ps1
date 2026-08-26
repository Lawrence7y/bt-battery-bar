# BtBatteryBar installer: copies the app to %LOCALAPPDATA%, enables autostart, launches it.
$ErrorActionPreference = "Stop"
$src = $PSScriptRoot
$dest = Join-Path $env:LOCALAPPDATA "BtBatteryBar"

# stop any running instance (single-instance guard also protects us)
Get-Process bt-battery-bar -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 600

New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item (Join-Path $src "bt-battery-bar.exe") $dest -Force
Copy-Item (Join-Path $src "README.md") $dest -Force

# autostart: same HKCU Run value the in-app toggle manages, so both stay consistent
Set-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "BtBatteryBar" -Value (Join-Path $dest "bt-battery-bar.exe")

Start-Process (Join-Path $dest "bt-battery-bar.exe")
Write-Host ""
Write-Host "Installed to: $dest"
Write-Host "Autostart:    enabled (HKCU Run)"
Write-Host "The battery strip should now appear near the tray clock."
