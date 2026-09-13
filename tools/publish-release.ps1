# Publish the GitHub release: push main + tags, then create the release with assets.
#
# Prerequisites (one-time, interactive - the maintainer must do this):
#   git push / git credential manager: needs the user's GitHub credentials
#   gh auth login                     : needs the user's GitHub account
# This script never handles credentials itself.
#
# ASCII-only on purpose (PowerShell 5.1 parses .ps1 with the ANSI code page unless
# the file has a UTF-8 BOM). The release notes are read from a UTF-8 markdown file:
#   docs/release-notes/v<version>.md
#
# Usage:
#   pwsh tools/publish-release.ps1                # push + release
#   pwsh tools/publish-release.ps1 -Draft         # create as draft first
#   pwsh tools/publish-release.ps1 -SkipPush
param(
    [switch]$Draft,
    [switch]$SkipPush,
    [string]$NotesFile = ""
)

$ErrorActionPreference = "Stop"
$script:RepoRoot = Split-Path -Parent $PSScriptRoot
$root = $script:RepoRoot
. (Join-Path $PSScriptRoot "verify-payload.ps1")

$version = Get-CrateVersion
$tag = "v$version"
Write-Host "release version: $version (tag $tag)"

# ---- sanity: repo ----
Push-Location $root
try {
    $dirty = (& git status --porcelain) | Where-Object { $_ -notmatch '^\?\?' }
    if ($dirty) {
        Write-Host "!! working tree has uncommitted changes:"
        $dirty | ForEach-Object { Write-Host "   $_" }
        throw "commit first, then publish"
    }
    $branch = (& git rev-parse --abbrev-ref HEAD).Trim()
    Write-Host "branch: $branch"

    if (-not $SkipPush) {
        Write-Host "==> git push origin $branch"
        Invoke-Native "git push failed (check credentials for the remote)" { git push origin $branch }
        Write-Host "==> git push tag $tag"
        Invoke-Native "git push tag failed" { git push origin $tag }
    }
}
finally { Pop-Location }

# ---- artifacts ----
$assets = @(
    (Join-Path $root "release\bt-battery-bar-$version-installer.zip"),
    (Join-Path $root "release\bt-battery-bar-$version-setup.zip")
)
foreach ($a in $assets) {
    if (-not (Test-Path $a)) { throw "missing artifact: $a (run tools/pack-release.ps1 first)" }
}

# ---- notes ----
if (-not $NotesFile) { $NotesFile = Join-Path $root "docs\release-notes\$tag.md" }
if (-not (Test-Path $NotesFile)) { throw "missing release notes: $NotesFile" }
Write-Host "notes: $NotesFile"

# ---- gh availability ----
$gh = Get-Command gh -ErrorAction SilentlyContinue
if (-not $gh) { throw "gh CLI not found" }
$auth = (& gh auth status 2>&1 | Out-String)
if ($auth -match "not logged into") {
    throw "gh is not authenticated. Run: gh auth login"
}

# ---- create the release ----
$args = @("release", "create", $tag, "--title", $tag, "--notes-file", $NotesFile)
if ($Draft) { $args += "--draft" }
foreach ($a in $assets) { $args += $a }
Write-Host "==> gh $($args -join ' ')"
Invoke-Native "gh release create failed" { & gh @args }

Write-Host ""
Write-Host "==> published:"
& gh release view $tag --json url, name, assets --template '{{.name}}  {{.url}}
{{range .assets}}  - {{.name}}  ({{.size}} bytes)
{{end}}'
