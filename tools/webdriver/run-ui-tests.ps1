#Requires -Version 7
<#
.SYNOPSIS
  Runs the WebDriver UI-flow tests (tests/ui) against the release app
  (ADR-0020): fetches the msedgedriver matching WebView2, starts
  tauri-driver, and runs the flows with an isolated app data directory.
  Logs (tauri-driver and app output, verbose msedgedriver log) go to the
  run's directory under .cache/ui-tests; their tails are printed on failure.
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
# msedgedriver through a wrapper that adds a verbose log; tauri-driver's
# output (and the app's, which inherits it) goes to files.
$driverLog = Join-Path $dir "msedgedriver.log"
$wrapper = Join-Path $dir "msedgedriver.cmd"
Set-Content $wrapper "@`"$driver`" %* --verbose `"--log-path=$driverLog`"" -Encoding ascii
$tdOut = Join-Path $dir "tauri-driver.out.log"
$tdErr = Join-Path $dir "tauri-driver.err.log"
$td = Start-Process $tauriDriver -ArgumentList "--native-driver", "`"$wrapper`"" -PassThru -WindowStyle Hidden `
  -RedirectStandardOutput $tdOut -RedirectStandardError $tdErr
try {
  Start-Sleep -Milliseconds 800
  $env:QUIRE_APP = $app
  $env:QUIRE_UI_DIR = $dir
  node --test --test-concurrency=1 (Join-Path $root "tests/ui/txt-flows.test.mjs")
  $code = $LASTEXITCODE
  if ($code -ne 0) {
    foreach ($log in $tdErr, $tdOut, $driverLog) {
      Write-Host "==== $log (last 60 lines)"
      if (Test-Path $log) { Get-Content $log -Tail 60 | Write-Host }
    }
    # Where WebView2 put its user data and DevToolsActivePort, versus what
    # msedgedriver waited for.
    if (Test-Path $driverLog) {
      Write-Host "==== msedgedriver.log: launch and environment lines"
      Select-String $driverLog -Pattern 'WEBVIEW2|user.data|DevToolsActivePort|Launching|scoped_dir|Starting' |
        ForEach-Object { Write-Host $_.Line }
    }
    Write-Host "==== DevToolsActivePort and EBWebView under the run dir and TEMP"
    foreach ($base in $dir, $env:TEMP) {
      Get-ChildItem $base -Recurse -Force -ErrorAction SilentlyContinue -Include DevToolsActivePort, EBWebView |
        ForEach-Object { Write-Host "$($_.FullName) $($_.LastWriteTime.ToString('o'))" }
    }
  }
} finally {
  Stop-Process $td -Force -ErrorAction SilentlyContinue
  Get-Process msedgedriver -ErrorAction SilentlyContinue | Stop-Process -Force
  $env:LOCALAPPDATA = $savedLocal
  Remove-Item Env:QUIRE_APP, Env:QUIRE_UI_DIR -ErrorAction SilentlyContinue
}
exit $code
