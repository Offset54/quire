#Requires -Version 7
<#
.SYNOPSIS
  Runs the M0 scenarios in the ref-sim profile and checks them against
  bench/budgets.json (ADR-0021).

.DESCRIPTION
  - Refuses to run on battery.
  - Pins the app and all its processes (WebView2 included) to the
    performance cores through tools/refsim, and tells the app the
    reference machine has 8 logical CPUs.
  - Uses a 1280x720 logical window: 1920x1080 device px at 150 %.
  - Measures D_io from cold sequential reads of a fixture.
  - Cold starts purge the app binary, the WebView2 runtime and the app's
    WebView2 data from the file cache first (files that other processes
    have mapped can't be purged; the count is reported).
  - Writes bench/results/m0/refsim-<timestamp>.json and prints a budget table.
#>
param(
  [switch] $NoBuild,
  [int] $WarmRuns = 7,
  [int] $ColdRuns = 5,
  [string] $Out
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$refsim = Join-Path $root "target/release/refsim.exe"
$app = Join-Path $root "target/bench/release/quire.exe"
$budgets = Get-Content (Join-Path $root "bench/budgets.json") -Raw | ConvertFrom-Json
$tmp = Join-Path $root ".cache/bench/refsim"
New-Item -ItemType Directory -Force $tmp | Out-Null

if (-not (Test-Path $refsim) -or -not $NoBuild) {
  cargo build -p quire-refsim --release
  if ($LASTEXITCODE -ne 0) { throw "refsim build failed" }
}
$info = & $refsim info | ConvertFrom-Json
if (-not $info.power.onAc) { throw "On battery: ref-sim runs are refused (ADR-0021). Plug in and retry." }
if (-not $NoBuild) {
  & (Join-Path $PSScriptRoot "run-app-bench.ps1") -Scenario none -BuildOnly | Out-Null
}

Write-Host "calibrating..."
$calibration = & $refsim calibrate | ConvertFrom-Json

# D_io: cold sequential read of the 300 MiB JSON fixture (median of 3).
$fixture = Join-Path $root ".cache/fixtures/single-line-300mib.json"
$ioRuns = @()
if (Test-Path $fixture) {
  foreach ($i in 1..3) {
    & $refsim purge $fixture | Out-Null
    $ioRuns += (& $refsim seqread $fixture | ConvertFrom-Json).mbPerSecond
  }
}
$coldReadMBps = if ($ioRuns) { ($ioRuns | Sort-Object)[1] } else { $null }
$dCpu = [double]$budgets.factors.cpu
$dIo = if ($coldReadMBps) { [Math]::Max(1.0, $coldReadMBps / [double]$budgets.factors.ioReferenceMBps) } else { $dCpu }

function Invoke-Scenario([string] $scenario, [string] $outFile) {
  $env:QUIRE_BENCH = $scenario
  $env:QUIRE_BENCH_OUT = $outFile
  $env:QUIRE_BENCH_VIEWPORT = "1280x720"
  Remove-Item $outFile -ErrorAction SilentlyContinue
  try {
    & $refsim run --pcores --logical 8 -- $app | Out-Null
  } finally {
    "QUIRE_BENCH", "QUIRE_BENCH_OUT", "QUIRE_BENCH_VIEWPORT" | ForEach-Object { Remove-Item "Env:$_" -ErrorAction SilentlyContinue }
  }
  if (-not (Test-Path $outFile)) { throw "scenario $scenario wrote no results" }
  $r = Get-Content $outFile -Raw | ConvertFrom-Json
  if ($r.error) { throw "scenario $scenario failed: $($r.error)" }
  $r
}

function Get-ColdPaths {
  $paths = @($app)
  $wv = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
  if ($wv) {
    $dir = "${env:ProgramFiles(x86)}\Microsoft\EdgeWebView\Application\$($wv.pv)"
    if (Test-Path $dir) { $paths += $dir }
  }
  $data = Join-Path $env:LOCALAPPDATA "dev.quire.app"
  if (Test-Path $data) { $paths += $data }
  $paths
}

function Get-Stats([double[]] $v) {
  $s = $v | Sort-Object
  [ordered]@{ n = $s.Count; p50 = $s[[int][Math]::Floor(($s.Count - 1) / 2)]; min = $s[0]; max = $s[-1]; runs = $v }
}

Write-Host "startup (warm x $WarmRuns)..."
Invoke-Scenario "startup" (Join-Path $tmp "warmup.json") | Out-Null
$warm = foreach ($i in 1..$WarmRuns) {
  $r = Invoke-Scenario "startup" (Join-Path $tmp "startup-warm-$i.json")
  [Math]::Round($r.firstFrameEpochMs - $r.processCreatedEpochMs, 1)
}

Write-Host "startup (cold x $ColdRuns)..."
$coldPaths = Get-ColdPaths
$purgeReport = $null
$cold = foreach ($i in 1..$ColdRuns) {
  $p = & $refsim purge @coldPaths | ConvertFrom-Json
  $others = @(Get-Process msedge, msedgewebview2 -ErrorAction SilentlyContinue).Count
  $purgeReport = [ordered]@{
    note = "pages mapped by other running Edge/WebView2 processes stay resident, so cold runs are partly warm when those exist"
    otherEdgeProcesses = $others; paths = $coldPaths; files = $p.files; purged = $p.purged; failed = $p.failed; sampleFailures = $p.failures | Select-Object -First 5 }
  $r = Invoke-Scenario "startup" (Join-Path $tmp "startup-cold-$i.json")
  [Math]::Round($r.firstFrameEpochMs - $r.processCreatedEpochMs, 1)
}

Write-Host "scroll-list..."
$scroll = Invoke-Scenario "scroll-list" (Join-Path $tmp "scroll-list.json")
$scroll.PSObject.Properties.Remove("snapshots")
$scrollLong = $scroll.steady.longTasks + $scroll.fling.longTasks + $scroll.drag.longTasks

$results = [ordered]@{
  profile = $budgets.profile
  date = (Get-Date).ToString("o")
  machine = [ordered]@{
    cpu = (Get-CimInstance Win32_Processor).Name
    ramGiB = [Math]::Round($info.memory.totalBytes / 1GB, 1)
    availableRamGiB = [Math]::Round($info.memory.availableBytes / 1GB, 1)
    performanceCores = $info.performanceCores
    power = $info.power
    webview2 = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue).pv
    commit = (git -C $root rev-parse --short HEAD)
  }
  refSim = [ordered]@{
    pinning = "job object, performance cores $($info.performanceCores -join ',')"
    logicalCpusReported = 8
    viewport = "1280x720 logical (1920x1080 device px at 150 %)"
    dCpu = $dCpu
    dIo = [Math]::Round($dIo, 3)
    coldReadMBps = $coldReadMBps
    coldStartPurge = $purgeReport
  }
  calibration = $calibration
  startup = [ordered]@{ warm = Get-Stats $warm; cold = Get-Stats $cold }
  scroll = [ordered]@{
    steady = $scroll.steady; fling = $scroll.fling; drag = $scroll.drag
    jumpToSharpMs = $scroll.jumpToSharpMs; firstPaintMs = $scroll.firstPaintMs
    longTasks = $scrollLong; env = $scroll.env
  }
}

function Get-Metric($obj, [string] $path) {
  $v = $obj
  foreach ($part in $path.Split(".")) { $v = $v.$part }
  $v
}

$rows = foreach ($b in ($budgets.budgets | Where-Object { $_.suite -eq "m0" })) {
  $value = [double](Get-Metric $results $b.metric)
  $factor = switch ($b.kind) { "cpu" { $dCpu } "io" { $dIo } "mixed" { [Math]::Max($dCpu, $dIo) } default { 1.0 } }
  $pass = if ($b.kind -eq "zero") { $value -eq 0 } else { $value * $factor -le $b.budget }
  [ordered]@{
    id = $b.id; value = $value; unit = $b.unit; kind = $b.kind; factor = [Math]::Round($factor, 3)
    effectiveBudget = if ($b.kind -eq "zero") { 0 } else { [Math]::Round($b.budget / $factor, 2) }
    budget = $b.budget; pass = $pass
  }
}
$results.budgets = $rows

if (-not $Out) {
  $Out = Join-Path $root ("bench/results/m0/refsim-" + (Get-Date -Format "yyyy-MM-dd-HHmmss") + ".json")
}
New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
$results | ConvertTo-Json -Depth 12 | Set-Content $Out
$rows | ForEach-Object { [pscustomobject]$_ } | Format-Table id, value, unit, kind, factor, effectiveBudget, budget, pass -AutoSize | Out-String -Width 200 | Write-Host
Write-Host "results: $Out"
if ($rows | Where-Object { -not $_.pass }) { exit 1 }
