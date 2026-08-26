# BtBatteryBar uninstaller.
$ErrorActionPreference = "SilentlyContinue"
Get-Process bt-battery-bar | Stop-Process -Force
Remove-ItemProperty -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run" -Name "BtBatteryBar"
Start-Sleep -Milliseconds 600
Remove-Item (Join-Path $env:LOCALAPPDATA "BtBatteryBar") -Recurse -Force
Write-Host "BtBatteryBar removed."
