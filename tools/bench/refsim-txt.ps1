#Requires -Version 7
<#
.SYNOPSIS
  Runs the TXT scenarios (M1) in the ref-sim profile and checks them
  against the "txt" budgets in bench/budgets.json (ADR-0021).

.DESCRIPTION
  - Refuses to run on battery; pins the app (and WebView2) to the
    performance cores and reports 8 logical CPUs; 1280x720 logical window.
  - Cold runs purge the fixture from the file cache first.
  - txt-2g: first paint, jumps before/after indexing, index time, inserts,
    typing, scrolling, memory (2 GiB fixture, cold).
  - txt-2g-search: literal search on the cold 2 GiB fixture.
  - txt-save: small edit + save of a 2 GiB copy, against the mean of raw
    sequential writes of the same size on the same volume taken right
    before and right after it ($SaveRuns runs, median ratio).
  - Frame rows are judged without platform frame stalls (ADR-0024): the
    scenarios report raw values, the stalls, and "own" values; budgets
    point at the own values, and the stalls are printed separately.
  - txt-300m: the 300 MiB one-line JSON.
  - launch-2g: the app started with the 2 GiB fixture (ADR-0022).
  Writes bench/results/m1/refsim-txt-<timestamp>.json.
#>
param(
  [switch] $NoBuild,
  [int] $Runs = 3,
  [int] $SaveRuns = 5,
  [string] $Out
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$refsim = Join-Path $root "target/release/refsim.exe"
$app = Join-Path $root "target/bench/release/quire.exe"
$budgets = Get-Content (Join-Path $root "bench/budgets.json") -Raw | ConvertFrom-Json
$fixtures = Join-Path $root ".cache/fixtures"
$big = Join-Path $fixtures "mixed-lines-2gib.txt"
$json = Join-Path $fixtures "single-line-300mib.json"
$tmp = Join-Path $root ".cache/bench/refsim-txt"
New-Item -ItemType Directory -Force $tmp | Out-Null
foreach ($f in $big, $json) { if (-not (Test-Path $f)) { throw "missing fixture $f (cargo run -p quire-fixtures --release)" } }

if (-not (Test-Path $refsim) -or -not $NoBuild) {
  cargo build -p quire-refsim --release
  if ($LASTEXITCODE -ne 0) { throw "refsim build failed" }
}
$info = & $refsim info | ConvertFrom-Json
if (-not $info.power.onAc) { throw "On battery: ref-sim runs are refused (ADR-0021). Plug in and retry." }
if (-not $NoBuild) {
  & (Join-Path $PSScriptRoot "run-app-bench.ps1") -Scenario none -BuildOnly | Out-Null
}

# D_io from cold sequential reads of the 300 MiB fixture (median of 3).
$ioRuns = foreach ($i in 1..3) {
  & $refsim purge $json | Out-Null
  (& $refsim seqread $json | ConvertFrom-Json).mbPerSecond
}
$coldReadMBps = ($ioRuns | Sort-Object)[1]
$dCpu = [double]$budgets.factors.cpu
$dIo = [Math]::Max(1.0, $coldReadMBps / [double]$budgets.factors.ioReferenceMBps)

# App data (journals, WebView2 profile) in a scratch directory.
$appData = Join-Path $tmp "appdata"
New-Item -ItemType Directory -Force $appData | Out-Null

function Invoke-Scenario([string] $scenario, [string] $file, [string[]] $appArgs = @()) {
  $outFile = Join-Path $tmp "$scenario-$([guid]::NewGuid().ToString('N').Substring(0, 8)).json"
  $saved = $env:LOCALAPPDATA
  $env:QUIRE_BENCH = $scenario
  $env:QUIRE_BENCH_OUT = $outFile
  $env:QUIRE_BENCH_VIEWPORT = "1280x720"
  $env:QUIRE_BENCH_FILE = $file
  $env:LOCALAPPDATA = $appData
  try {
    & $refsim run --pcores --logical 8 -- $app @appArgs | Out-Null
  } finally {
    $env:LOCALAPPDATA = $saved
    "QUIRE_BENCH", "QUIRE_BENCH_OUT", "QUIRE_BENCH_VIEWPORT", "QUIRE_BENCH_FILE" | ForEach-Object { Remove-Item "Env:$_" -ErrorAction SilentlyContinue }
  }
  if (-not (Test-Path $outFile)) { throw "scenario $scenario wrote no results" }
  $r = Get-Content $outFile -Raw | ConvertFrom-Json
  if ($r.error) { throw "scenario $scenario failed: $($r.error)" }
  $r
}

function Median([object[]] $runs, [scriptblock] $metric) {
  $v = @($runs | ForEach-Object { [double](& $metric $_) } | Sort-Object)
  $v[[int][Math]::Floor(($v.Count - 1) / 2)]
}

Write-Host "txt-2g (cold) x $Runs..."
$t2g = foreach ($i in 1..$Runs) {
  & $refsim purge $big | Out-Null
  Invoke-Scenario "txt-2g" $big
}
Write-Host "txt-2g-search (cold) x $Runs..."
$search = foreach ($i in 1..$Runs) {
  & $refsim purge $big | Out-Null
  Invoke-Scenario "txt-2g-search" $big
}
Write-Host "txt-300m (cold) x $Runs..."
$t300 = foreach ($i in 1..$Runs) {
  & $refsim purge $json | Out-Null
  Invoke-Scenario "txt-300m" $json
}
Write-Host "launch-2g (cold file) x $Runs..."
$launch = foreach ($i in 1..$Runs) {
  & $refsim purge $big | Out-Null
  $r = Invoke-Scenario "launch-2g" $big @($big)
  $raw = $r.firstContentEpochMs - $r.processCreatedEpochMs
  [ordered]@{ ms = [Math]::Round($raw, 1); ownMs = [Math]::Round($raw - $r.stallMs, 1); stallMs = $r.stallMs; firstFrameWaitMs = $r.firstFrameWaitMs }
}
Write-Host "txt-save x $SaveRuns..."
# Save against the mean of raw sequential writes of the same size taken
# right before and right after it (M1_PLAN, decided 2026-10-12): raw writes
# alone vary 1.0-2.1 s on this SSD.
$copy = Join-Path $tmp "txt-2g-copy.txt"
$seqFile = Join-Path $tmp "seqwrite.bin"
$saveBytes = (Get-Item $big).Length + "SAVE-BENCH ".Length
$saves = foreach ($i in 1..$SaveRuns) {
  Copy-Item $big $copy -Force
  $before = & $refsim seqwrite $seqFile $saveBytes | ConvertFrom-Json
  $s = Invoke-Scenario "txt-save" $copy
  $after = & $refsim seqwrite $seqFile $s.bytes | ConvertFrom-Json
  $rawMs = ($before.seconds + $after.seconds) * 500
  [ordered]@{
    saveMs = $s.saveMs; seqwriteBeforeMs = [Math]::Round($before.seconds * 1000); seqwriteAfterMs = [Math]::Round($after.seconds * 1000)
    ratio = [Math]::Round($s.saveMs / $rawMs, 3); bytes = $s.bytes; ok = $s.ok
  }
}
Remove-Item $copy, $seqFile -ErrorAction SilentlyContinue

# The median run per scenario, chosen by its headline metric.
function Pick([object[]] $runs, [scriptblock] $key) {
  $sorted = @($runs | Sort-Object { [double](& $key $_) })
  $sorted[[int][Math]::Floor(($sorted.Count - 1) / 2)]
}
$t2gMid = Pick $t2g { $_.ownFirstPaintMs }
$searchMid = Pick $search { $_.totalMs }
$t300Mid = Pick $t300 { $_.ownFirstPaintMs }
$saveMid = Pick $saves { $_.ratio }
$long = ($t2g | ForEach-Object { $_.typing.longTasks + $_.steady.longTasks + $_.fling.longTasks } | Measure-Object -Sum).Sum +
  ($t300 | ForEach-Object { $_.longTasks } | Measure-Object -Sum).Sum

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
  txt2g = $t2gMid
  txt2gAll = [ordered]@{
    firstPaintMs = @($t2g | ForEach-Object { $_.firstPaintMs }); ownFirstPaintMs = @($t2g | ForEach-Object { $_.ownFirstPaintMs }); indexMs = @($t2g | ForEach-Object { $_.indexMs })
    typingP95 = @($t2g | ForEach-Object { $_.typing.latencyMs.p95 }); insertP95 = @($t2g | ForEach-Object { $_.insertMs.p95 })
  }
  search = $searchMid
  searchAll = @($search | ForEach-Object { [ordered]@{ totalMs = $_.totalMs; firstHitMs = $_.firstHitMs; matches = $_.matches } })
  txt300m = $t300Mid
  txt300mAll = @($t300 | ForEach-Object { [ordered]@{ firstPaintMs = $_.firstPaintMs; caretMax = $_.caretMs.max; wrapMax = $_.wrapToggleMs.max } })
  save = $saveMid
  saveAll = $saves
  launch = [ordered]@{
    runs = $launch; p50 = (Median $launch { $_.ms }); ownP50 = (Median $launch { $_.ownMs })
  }
  # Platform frame stalls (ADR-0024), reported, not judged.
  platformStalls = [ordered]@{
    txt2gFirstPaint = @($t2g | ForEach-Object { [ordered]@{ rawMs = $_.firstPaintMs; ownMs = $_.ownFirstPaintMs; firstFrameWaitMs = $_.firstFrameWaitMs; firstFrameWaitStallMs = $_.firstFrameWaitStallMs; stalls = $_.firstPaintStalls } })
    txt300mFirstPaint = @($t300 | ForEach-Object { [ordered]@{ rawMs = $_.firstPaintMs; ownMs = $_.ownFirstPaintMs; firstFrameWaitMs = $_.firstFrameWaitMs; firstFrameWaitStallMs = $_.firstFrameWaitStallMs; stalls = $_.firstPaintStalls } })
    steady = @($t2g | ForEach-Object { [ordered]@{ rawMax = $_.steady.frameIntervalMs.max; ownMax = $_.steady.ownFrameIntervalMs.max; stalls = $_.steady.platformStalls.count; worstStallMs = $_.steady.platformStalls.worstMs } })
    fling = @($t2g | ForEach-Object { [ordered]@{ rawMax = $_.fling.frameIntervalMs.max; ownMax = $_.fling.ownFrameIntervalMs.max; stalls = $_.fling.platformStalls.count; worstStallMs = $_.fling.platformStalls.worstMs } })
    wrappedScroll = @($t300 | ForEach-Object { [ordered]@{ rawMax = $_.wrappedScroll.frameIntervalMs.max; ownMax = $_.wrappedScroll.ownFrameIntervalMs.max; stalls = $_.wrappedScroll.platformStalls.count; worstStallMs = $_.wrappedScroll.platformStalls.worstMs } })
    scrollAfterIdle = @($t2g | ForEach-Object { , @($_.scrollAfterIdle) })
  }
  longTasks = $long
}

function Get-Metric($obj, [string] $path) {
  $v = $obj
  foreach ($part in $path.Split(".")) { $v = $v.$part }
  $v
}

$rows = foreach ($b in ($budgets.budgets | Where-Object { $_.suite -eq "txt" })) {
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
  $Out = Join-Path $root ("bench/results/m1/refsim-txt-" + (Get-Date -Format "yyyy-MM-dd-HHmmss") + ".json")
}
New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
$results | ConvertTo-Json -Depth 14 | Set-Content $Out
$rows | ForEach-Object { [pscustomobject]$_ } | Format-Table id, value, unit, kind, factor, effectiveBudget, budget, pass -AutoSize | Out-String -Width 200 | Write-Host
Write-Host ("launch-2g first content (process creation → rows on screen): " + (($launch | ForEach-Object { "$($_.ms) (own $($_.ownMs))" }) -join ", ") + " ms")
Write-Host "Platform frame stalls (ADR-0024; reported, not judged):"
$ps = $results.platformStalls
Write-Host ("  2 GB first paint raw/own: " + (($ps.txt2gFirstPaint | ForEach-Object { "$($_.rawMs)/$($_.ownMs)" }) -join ", ") + " ms")
Write-Host ("  300 MB first paint raw/own: " + (($ps.txt300mFirstPaint | ForEach-Object { "$($_.rawMs)/$($_.ownMs)" }) -join ", ") + " ms")
foreach ($k in "steady", "fling", "wrappedScroll") {
  Write-Host ("  $k max frame raw/own (stalls): " + (($ps.$k | ForEach-Object { "$($_.rawMax)/$($_.ownMax) ($($_.stalls))" }) -join ", ") + " ms")
}
Write-Host "results: $Out"
if ($rows | Where-Object { -not $_.pass }) { exit 1 }
