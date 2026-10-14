#Requires -Version 7
<#
.SYNOPSIS
  Runs the Markdown scenarios (M2) in the ref-sim profile and checks them
  against the "md" budgets in bench/budgets.json (ADR-0021).

.DESCRIPTION
  - Refuses to run on battery; pins the app (and WebView2) to the
    performance cores and reports 8 logical CPUs; 1280x720 logical window.
  - md-20m: the 20 MiB fixture opened cold (purged from the file cache) in
    split view: first paint of both panes, long tasks while opening, the
    background index, memory.
  - md-20m-edit: keystroke → preview update at the plan's positions, typing
    in the source view, preview scrolling steady and fling.
  - Frame rows are judged without platform frame stalls (ADR-0024): the
    scenarios report raw values, the stalls, and "own" values; budgets point
    at the own values, and the stalls are printed separately.
  Writes bench/results/m2/refsim-md-<timestamp>.json.
#>
param(
  [switch] $NoBuild,
  [int] $Runs = 3,
  [string] $Out
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$refsim = Join-Path $root "target/release/refsim.exe"
$app = Join-Path $root "target/bench/release/quire.exe"
$budgets = Get-Content (Join-Path $root "bench/budgets.json") -Raw | ConvertFrom-Json
$fixture = Join-Path $root ".cache/fixtures/document-20mib.md"
$tmp = Join-Path $root ".cache/bench/refsim-md"
New-Item -ItemType Directory -Force $tmp | Out-Null
if (-not (Test-Path $fixture)) { throw "missing fixture $fixture (cargo run -p quire-fixtures --release)" }

if (-not (Test-Path $refsim) -or -not $NoBuild) {
  cargo build -p quire-refsim --release
  if ($LASTEXITCODE -ne 0) { throw "refsim build failed" }
}
$info = & $refsim info | ConvertFrom-Json
if (-not $info.power.onAc) { throw "On battery: ref-sim runs are refused (ADR-0021). Plug in and retry." }
if (-not $NoBuild) {
  & (Join-Path $PSScriptRoot "run-app-bench.ps1") -Scenario none -BuildOnly | Out-Null
}

# D_io from cold sequential reads of the fixture (median of 3).
$ioRuns = foreach ($i in 1..3) {
  & $refsim purge $fixture | Out-Null
  (& $refsim seqread $fixture | ConvertFrom-Json).mbPerSecond
}
$coldReadMBps = ($ioRuns | Sort-Object)[1]
$dCpu = [double]$budgets.factors.cpu
$dIo = [Math]::Max(1.0, $coldReadMBps / [double]$budgets.factors.ioReferenceMBps)

# App data (journals, WebView2 profile) in a scratch directory.
$appData = Join-Path $tmp "appdata"
New-Item -ItemType Directory -Force $appData | Out-Null

function Invoke-Scenario([string] $scenario, [string] $file) {
  $outFile = Join-Path $tmp "$scenario-$([guid]::NewGuid().ToString('N').Substring(0, 8)).json"
  $saved = $env:LOCALAPPDATA
  $env:QUIRE_BENCH = $scenario
  $env:QUIRE_BENCH_OUT = $outFile
  $env:QUIRE_BENCH_VIEWPORT = "1280x720"
  $env:QUIRE_BENCH_FILE = $file
  $env:LOCALAPPDATA = $appData
  try {
    & $refsim run --pcores --logical 8 -- $app | Out-Null
  } finally {
    $env:LOCALAPPDATA = $saved
    "QUIRE_BENCH", "QUIRE_BENCH_OUT", "QUIRE_BENCH_VIEWPORT", "QUIRE_BENCH_FILE" | ForEach-Object { Remove-Item "Env:$_" -ErrorAction SilentlyContinue }
  }
  if (-not (Test-Path $outFile)) { throw "scenario $scenario wrote no results" }
  $r = Get-Content $outFile -Raw | ConvertFrom-Json
  if ($r.error) { throw "scenario $scenario failed: $($r.error)" }
  $r
}

Write-Host "md-20m (cold) x $Runs..."
$open = foreach ($i in 1..$Runs) {
  & $refsim purge $fixture | Out-Null
  Invoke-Scenario "md-20m" $fixture
}
Write-Host "md-20m-edit x $Runs..."
$edit = foreach ($i in 1..$Runs) {
  Invoke-Scenario "md-20m-edit" $fixture
}

# The median run per scenario, chosen by its headline metric.
function Pick([object[]] $runs, [scriptblock] $key) {
  $sorted = @($runs | Sort-Object { [double](& $key $_) })
  $sorted[[int][Math]::Floor(($sorted.Count - 1) / 2)]
}
$openMid = Pick $open { $_.ownFirstPaintMs }
$editMid = Pick $edit { $_.keystrokeToPreview.ownMs.p95 }
$long = ($open | ForEach-Object { $_.firstPaintLongTasks } | Measure-Object -Sum).Sum +
  ($edit | ForEach-Object { $_.longTasks } | Measure-Object -Sum).Sum

function Positions($r) {
  $o = [ordered]@{}
  foreach ($p in $r.keystrokeToPreview.positions.PSObject.Properties) {
    $o[$p.Name] = [ordered]@{
      p50 = $p.Value.ms.p50; p95 = $p.Value.ms.p95; max = $p.Value.ms.max
      ownP95 = $p.Value.ownMs.p95; ownMax = $p.Value.ownMs.max
      arrivedP50 = $p.Value.arrivedMs.p50
      correctionsP50 = $p.Value.correctionsMs.p50; correctionsMax = $p.Value.correctionsMs.max
    }
  }
  $o
}

$results = [ordered]@{
  profile = $budgets.profile
  date = (Get-Date).ToString("o")
  machine = [ordered]@{
    cpu = (Get-CimInstance Win32_Processor).Name
    ramGiB = [Math]::Round($info.memory.totalBytes / 1GB, 1)
    performanceCores = $info.performanceCores
    power = $info.power
    webview2 = (Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue).pv
    commit = (git -C $root rev-parse --short HEAD)
  }
  refSim = [ordered]@{ pinning = "job object, performance cores"; logicalCpusReported = 8; viewport = "1280x720 logical (1920x1080 device px at 150 %)"; dCpu = $dCpu; dIo = [Math]::Round($dIo, 3); coldReadMBps = $coldReadMBps }
  runs = $Runs
  md20m = $openMid
  md20mAll = [ordered]@{
    firstPaintMs = @($open | ForEach-Object { $_.firstPaintMs }); ownFirstPaintMs = @($open | ForEach-Object { $_.ownFirstPaintMs })
    indexMs = @($open | ForEach-Object { $_.indexMs }); longTasks = @($open | ForEach-Object { $_.firstPaintLongTasks })
    privateGrowthMiB = @($open | ForEach-Object { $_.memory.privateGrowthMiB })
  }
  md20mEdit = $editMid
  md20mEditAll = @($edit | ForEach-Object {
      [ordered]@{
        keystrokeP95 = $_.keystrokeToPreview.ms.p95; keystrokeMax = $_.keystrokeToPreview.ms.max
        keystrokeOwnP95 = $_.keystrokeToPreview.ownMs.p95; keystrokeOwnMax = $_.keystrokeToPreview.ownMs.max
        positions = (Positions $_)
        typingP95 = $_.typing.latencyMs.p95; typingP99 = $_.typing.latencyMs.p99
        typingOwnP95 = $_.typing.ownLatencyMs.p95; typingOwnP99 = $_.typing.ownLatencyMs.p99
        steadyOwnMax = $_.previewSteady.ownFrameIntervalMs.max; flingOwnMax = $_.previewFling.ownFrameIntervalMs.max
        longTasks = $_.longTasks
      }
    })
  # Platform frame stalls (ADR-0024), reported, not judged.
  platformStalls = [ordered]@{
    firstPaint = @($open | ForEach-Object { [ordered]@{ rawMs = $_.firstPaintMs; ownMs = $_.ownFirstPaintMs; firstFrameWaitStallMs = $_.firstFrameWaitStallMs; stalls = $_.firstPaintStalls } })
    steady = @($edit | ForEach-Object { [ordered]@{ rawMax = $_.previewSteady.frameIntervalMs.max; ownMax = $_.previewSteady.ownFrameIntervalMs.max; stalls = $_.previewSteady.platformStalls.count; worstStallMs = $_.previewSteady.platformStalls.worstMs } })
    keystroke = @($edit | ForEach-Object { [ordered]@{ rawP95 = $_.keystrokeToPreview.ms.p95; rawMax = $_.keystrokeToPreview.ms.max; ownP95 = $_.keystrokeToPreview.ownMs.p95; ownMax = $_.keystrokeToPreview.ownMs.max } })
    typing = @($edit | ForEach-Object { [ordered]@{ rawP99 = $_.typing.latencyMs.p99; rawMax = $_.typing.latencyMs.max; ownP99 = $_.typing.ownLatencyMs.p99; ownMax = $_.typing.ownLatencyMs.max } })
    fling = @($edit | ForEach-Object { [ordered]@{ rawMax = $_.previewFling.frameIntervalMs.max; ownMax = $_.previewFling.ownFrameIntervalMs.max; stalls = $_.previewFling.platformStalls.count; worstStallMs = $_.previewFling.platformStalls.worstMs } })
  }
  longTasks = $long
}

function Get-Metric($obj, [string] $path) {
  $v = $obj
  foreach ($part in $path.Split(".")) { $v = $v.$part }
  $v
}

$rows = foreach ($b in ($budgets.budgets | Where-Object { $_.suite -eq "md" })) {
  $raw = Get-Metric $results $b.metric
  $measured = $null -ne $raw -and -not ($raw -is [string])
  $value = if ($measured) { [double]$raw } else { [double]::NaN }
  $factor = switch ($b.kind) { "cpu" { $dCpu } "io" { $dIo } "mixed" { [Math]::Max($dCpu, $dIo) } default { 1.0 } }
  $pass = if (-not $measured -or [double]::IsNaN($value)) { $false } elseif ($b.kind -eq "zero") { $value -eq 0 } else { $value * $factor -le $b.budget }
  [ordered]@{
    id = $b.id; value = $value; unit = $b.unit; kind = $b.kind; factor = [Math]::Round($factor, 3)
    effectiveBudget = if ($b.kind -eq "zero") { 0 } else { [Math]::Round($b.budget / $factor, 2) }
    budget = $b.budget; pass = $pass; measured = $measured
  }
}
$results.budgets = $rows

if (-not $Out) {
  $Out = Join-Path $root ("bench/results/m2/refsim-md-" + (Get-Date -Format "yyyy-MM-dd-HHmmss") + ".json")
}
New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
$results | ConvertTo-Json -Depth 14 | Set-Content $Out
$rows | ForEach-Object { [pscustomobject]$_ } | Format-Table id, value, unit, kind, factor, effectiveBudget, budget, pass -AutoSize | Out-String -Width 200 | Write-Host
Write-Host "Keystroke → preview by position (median run; raw p50/p95/max, own p95/max, corrections p50/max):"
foreach ($p in (Positions $editMid).GetEnumerator()) {
  $v = $p.Value
  Write-Host ("  {0}: {1}/{2}/{3} ms, own {4}/{5}; corrections {6}/{7}" -f $p.Key, $v.p50, $v.p95, $v.max, $v.ownP95, $v.ownMax, $v.correctionsP50, $v.correctionsMax)
}
Write-Host "Platform frame stalls (ADR-0024; reported, not judged):"
$ps = $results.platformStalls
Write-Host ("  first paint raw/own: " + (($ps.firstPaint | ForEach-Object { "$($_.rawMs)/$($_.ownMs)" }) -join ", ") + " ms")
Write-Host ("  keystroke p95/max raw → own: " + (($ps.keystroke | ForEach-Object { "$($_.rawP95)/$($_.rawMax) → $($_.ownP95)/$($_.ownMax)" }) -join ", ") + " ms")
Write-Host ("  typing p99/max raw → own: " + (($ps.typing | ForEach-Object { "$($_.rawP99)/$($_.rawMax) → $($_.ownP99)/$($_.ownMax)" }) -join ", ") + " ms")
foreach ($k in "steady", "fling") {
  Write-Host ("  $k max frame raw/own (stalls): " + (($ps.$k | ForEach-Object { "$($_.rawMax)/$($_.ownMax) ($($_.stalls))" }) -join ", ") + " ms")
}
Write-Host "results: $Out"
if ($rows | Where-Object { -not $_.pass }) { exit 1 }
