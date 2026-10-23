#Requires -Version 7
<#
.SYNOPSIS
  Runs the WebDriver UI-flow tests (tests/ui) against the release app
  (ADR-0020): fetches the msedgedriver matching WebView2, starts
  tauri-driver, and runs the flows with an isolated app data directory.
  Logs (tauri-driver and app output, verbose msedgedriver log) go to the
  run's directory under .cache/ui-tests; their tails are printed on failure.
#>
param([switch] $NoBuild, [string[]] $Tests = @("txt-flows", "md-flows", "pdf-flows"))
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
# The app inherits tauri-driver's environment. The PDF flows crash a worker
# on purpose (pdf_crash_worker refuses without this).
$env:QUIRE_PDF_FAULTS = "1"
# msedgedriver through a wrapper that adds a verbose log; tauri-driver's
# output (and the app's, which inherits it) goes to files.
$driverLog = Join-Path $dir "msedgedriver.log"
$wrapper = Join-Path $dir "msedgedriver.cmd"
Set-Content $wrapper "@`"$driver`" %* --verbose `"--log-path=$driverLog`"" -Encoding ascii
$tdOut = Join-Path $dir "tauri-driver.out.log"
$tdErr = Join-Path $dir "tauri-driver.err.log"
$td = Start-Process $tauriDriver -ArgumentList "--native-driver", "`"$wrapper`"" -PassThru -WindowStyle Hidden `
  -RedirectStandardOutput $tdOut -RedirectStandardError $tdErr
# While the first session starts: which app and WebView2 processes run, with
# what command lines, and what msedgedriver's scoped dirs hold.
$probeLog = Join-Path $dir "process-probe.log"
$probe = Start-ThreadJob -ArgumentList $probeLog -ScriptBlock {
  param($out)
  "user: $([Security.Principal.WindowsIdentity]::GetCurrent().Name)" | Add-Content $out
  $prev = 0
  foreach ($t in 5, 15) {
    Start-Sleep -Seconds ($t - $prev)
    $prev = $t
    "---- t+$t s" | Add-Content $out
    $procs = Get-CimInstance Win32_Process -Filter "Name='quire.exe' OR Name='msedgewebview2.exe' OR Name='msedgedriver.exe'" |
      Where-Object { $_.Name -ne 'msedgewebview2.exe' -or $_.CommandLine -match '--webview-exe-name=quire\.exe' }
    foreach ($p in $procs) {
      $owner = Invoke-CimMethod -InputObject $p -MethodName GetOwner -ErrorAction SilentlyContinue
      "$($p.ProcessId) <- $($p.ParentProcessId) $($p.Name) owner=$($owner.Domain)\$($owner.User)" | Add-Content $out
      # The WebView2 browser process: every switch, and its listening ports.
      if ($p.Name -eq 'msedgewebview2.exe' -and $p.CommandLine -notmatch ' --type=' -and
        $p.CommandLine -match '--webview-exe-name=quire\.exe') {
        $p.CommandLine -split ' (?=--)' | ForEach-Object { "    $_" } | Add-Content $out
        Get-NetTCPConnection -OwningProcess $p.ProcessId -State Listen -ErrorAction SilentlyContinue |
          ForEach-Object { "    listening $($_.LocalAddress):$($_.LocalPort)" } | Add-Content $out
        if ($p.CommandLine -match '--user-data-dir="?([^"]+?)"?( --|$)') {
          Get-ChildItem $Matches[1] -Force -ErrorAction SilentlyContinue | ForEach-Object { "    udd: $($_.Name)" } | Add-Content $out
          Get-ChildItem (Split-Path $Matches[1]) -Force -ErrorAction SilentlyContinue | ForEach-Object { "    scoped: $($_.Name)" } | Add-Content $out
        }
      }
    }
  }
}
try {
  Start-Sleep -Milliseconds 800
  $env:QUIRE_APP = $app
  $env:QUIRE_UI_DIR = $dir
  $files = @($Tests | ForEach-Object { Join-Path $root "tests/ui/$_.test.mjs" })
  node --test --test-concurrency=1 $files
  $code = $LASTEXITCODE
  if ($code -ne 0) {
    $probe | Wait-Job | Out-Null
    Write-Host "==== $probeLog"
    if (Test-Path $probeLog) { Get-Content $probeLog | ForEach-Object { Write-Host ($_.Substring(0, [Math]::Min(600, $_.Length))) } }
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
  Remove-Job $probe -Force -ErrorAction SilentlyContinue
  Stop-Process $td -Force -ErrorAction SilentlyContinue
  Get-Process msedgedriver -ErrorAction SilentlyContinue | Stop-Process -Force
  $env:LOCALAPPDATA = $savedLocal
  Remove-Item Env:QUIRE_PDF_FAULTS -ErrorAction SilentlyContinue
  Remove-Item Env:QUIRE_APP, Env:QUIRE_UI_DIR -ErrorAction SilentlyContinue
}
exit $code
