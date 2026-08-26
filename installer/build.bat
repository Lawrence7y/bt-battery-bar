@echo off
setlocal
rem ============================================================
rem  BtBatteryBar GUI installer builder
rem  Requires: .NET Framework 4.x (csc.exe) - no internet needed
rem  Outputs:  BtBatteryBar-Setup.exe  (self-contained, embeds app+uninstaller)
rem            BtBatteryBar-Uninstall.exe
rem ============================================================
set "HERE=%~dp0"
set "ROOT=%HERE%.."
set "CSC=%WINDIR%\Microsoft.NET\Framework64\v4.0.30319\csc.exe"
if not exist "%CSC%" set "CSC=%WINDIR%\Microsoft.NET\Framework\v4.0.30319\csc.exe"
if not exist "%CSC%" (
  echo [ERROR] csc.exe not found. Install .NET Framework 4.x.
  exit /b 1
)

echo [1/2] Building BtBatteryBar-Uninstall.exe ...
"%CSC%" /nologo /target:winexe /optimize+ /win32icon:"%HERE%assets\icon.ico" /out:"%HERE%BtBatteryBar-Uninstall.exe" "%HERE%Uninstall.cs"
if errorlevel 1 goto :fail

echo [2/2] Building BtBatteryBar-Setup.exe (embedding app + uninstaller) ...
"%CSC%" /nologo /target:winexe /optimize+ /win32icon:"%HERE%assets\icon.ico" /resource:"%ROOT%\dist\bt-battery-bar.exe" /resource:"%HERE%BtBatteryBar-Uninstall.exe" /out:"%HERE%BtBatteryBar-Setup.exe" "%HERE%Setup.cs"
if errorlevel 1 goto :fail

echo.
echo Build OK:
echo   %HERE%BtBatteryBar-Setup.exe
echo   %HERE%BtBatteryBar-Uninstall.exe
goto :eof

:fail
echo.
echo [ERROR] Build failed.
exit /b 1
