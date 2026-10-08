#Requires -Version 7
<#
.SYNOPSIS
  Runs the WebDriver UI-flow tests (tests/ui) against the release app
  (ADR-0020): fetches the msedgedriver matching WebView2, starts
  tauri-driver, and runs the flows with an isolated app data directory.
#>
param([switch] $NoBuild)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$app = Join-Path $root "target/release/quire.exe"
if (-not $NoBuild) {
  Push-Location (Join-Path $root "apps/desktop")
  try {
    cargo tauri build --no-bundle
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
  } finally { Pop-Location }
}
$driver = & (Join-Path $PSScriptRoot "fetch-msedgedriver.ps1") | Select-Object -Last 1
$dir = Join-Path $root ".cache/ui-tests/$(Get-Date -Format yyyyMMdd-HHmmss)"
New-Item -ItemType Directory -Force $dir | Out-Null
$tauriDriver = (Get-Command tauri-driver -ErrorAction SilentlyContinue).Source
if (-not $tauriDriver) { $tauriDriver = Join-Path $env:USERPROFILE ".cargo/bin/tauri-driver.exe" }

$savedLocal = $env:LOCALAPPDATA
$env:LOCALAPPDATA = Join-Path $dir "appdata"
New-Item -ItemType Directory -Force $env:LOCALAPPDATA | Out-Null
$td = Start-Process $tauriDriver -ArgumentList "--native-driver", "`"$driver`"" -PassThru -WindowStyle Hidden
try {
  Start-Sleep -Milliseconds 800
  $env:QUIRE_APP = $app
  $env:QUIRE_UI_DIR = $dir
  node --test --test-concurrency=1 (Join-Path $root "tests/ui/txt-flows.test.mjs")
  $code = $LASTEXITCODE
} finally {
  Stop-Process $td -Force -ErrorAction SilentlyContinue
  Get-Process msedgedriver -ErrorAction SilentlyContinue | Stop-Process -Force
  $env:LOCALAPPDATA = $savedLocal
  Remove-Item Env:QUIRE_APP, Env:QUIRE_UI_DIR -ErrorAction SilentlyContinue
}
exit $code
