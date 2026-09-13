# Shared payload verification for the packaging scripts (pack-msix.ps1 / pack-release.ps1).
#
# ASCII-only on purpose: Windows PowerShell 5.1 parses .ps1 with the ANSI code page
# unless the file carries a UTF-8 BOM, so non-ASCII comments are a parsing hazard.
#
# Two regressions this guards against (both shipped once):
#   1. a console-subsystem exe -> double-click opens a terminal window
#   2. a dynamically linked CRT -> the app will not start without the VC++ runtime
function Assert-Payload {
    param([Parameter(Mandatory = $true)][string]$Exe)

    if (-not (Test-Path $Exe)) { throw "payload not found: $Exe" }

    $bytes = [System.IO.File]::ReadAllBytes($Exe)
    $peOff = [BitConverter]::ToInt32($bytes, 0x3C)
    $subsystem = [BitConverter]::ToUInt16($bytes, $peOff + 24 + 68)
    $verdict = if ($subsystem -eq 2) { "WINDOWS_GUI, ok" } else { "NOT GUI" }
    Write-Host ("    PE Subsystem = {0} ({1})" -f $subsystem, $verdict)
    if ($subsystem -ne 2) {
        throw "payload is a console app (Subsystem=$subsystem): double-click opens a console window. Check that src/main.rs has #![windows_subsystem = `"windows`"]"
    }

    $ascii = [System.Text.Encoding]::ASCII.GetString($bytes)
    foreach ($dll in @("VCRUNTIME140.dll", "MSVCP140.dll")) {
        if ($ascii.Contains($dll)) {
            throw "payload depends on ${dll}: +crt-static from .cargo/config.toml did not apply; the app will not start without the VC++ runtime"
        }
    }
    Write-Host "    static CRT ok (no VCRUNTIME140 / MSVCP140 dependency)"
}

# Native tools write progress to stderr; with $ErrorActionPreference = 'Stop',
# PowerShell 5.1 turns that into a terminating NativeCommandError.
function Invoke-Native {
    param(
        [string]$FailMessage,
        [scriptblock]$Body,
        # makeappx logs one line per payload file; keep only the summary/error lines.
        [string]$OnlyMatching = ""
    )
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    # 2>&1 keeps the tool's progress visible without PowerShell raising/writing a
    # NativeCommandError record for it (cargo/makeappx/signtool log to stderr).
    try {
        & $Body 2>&1 | ForEach-Object {
            if (-not $OnlyMatching -or "$_" -match $OnlyMatching) { Write-Host "$_" }
        }
    } finally { $ErrorActionPreference = $prev }
    if ($LASTEXITCODE -ne 0) { throw ("$FailMessage (exit code $LASTEXITCODE)") }
}

# Version from Cargo.toml, optionally padded to 4 parts for MSIX.
function Get-CrateVersion {
    param([switch]$FourParts)
    $toml = Get-Content (Join-Path $script:RepoRoot "Cargo.toml") -Raw
    if ($toml -notmatch '(?m)^\s*version\s*=\s*"([^"]+)"') { throw "cannot read version from Cargo.toml" }
    $v = $Matches[1]
    if (-not $FourParts) { return $v }
    $parts = @($v.Split('.'))
    while ($parts.Count -lt 4) { $parts += "0" }
    return ($parts[0..3] -join '.')
}
