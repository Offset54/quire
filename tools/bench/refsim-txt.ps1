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
  - txt-save: small edit + save of a 2 GiB copy, against a raw sequential
    write of the same size on the same volume.
  - txt-300m: the 300 MiB one-line JSON.
  - launch-2g: the app started with the 2 GiB fixture (ADR-0022).
  Writes bench/results/m1/refsim-txt-<timestamp>.json.
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
  [Math]::Round($r.firstContentEpochMs - $r.processCreatedEpochMs, 1)
}
Write-Host "txt-save..."
$copy = Join-Path $tmp "txt-2g-copy.txt"
$saves = foreach ($i in 1..$Runs) {
  Copy-Item $big $copy -Force
  $s = Invoke-Scenario "txt-save" $copy
  $w = & $refsim seqwrite (Join-Path $tmp "seqwrite.bin") $s.bytes | ConvertFrom-Json
  [ordered]@{ saveMs = $s.saveMs; seqwriteMs = [Math]::Round($w.seconds * 1000); ratio = [Math]::Round($s.saveMs / ($w.seconds * 1000), 3); bytes = $s.bytes; ok = $s.ok }
}
Remove-Item $copy -ErrorAction SilentlyContinue

# The median run per scenario, chosen by its headline metric.
function Pick([object[]] $runs, [scriptblock] $key) {
  $sorted = @($runs | Sort-Object { [double](& $key $_) })
  $sorted[[int][Math]::Floor(($sorted.Count - 1) / 2)]
}
$t2gMid = Pick $t2g { $_.firstPaintMs }
$searchMid = Pick $search { $_.totalMs }
$t300Mid = Pick $t300 { $_.firstPaintMs }
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
    firstPaintMs = @($t2g | ForEach-Object { $_.firstPaintMs }); indexMs = @($t2g | ForEach-Object { $_.indexMs })
    typingP95 = @($t2g | ForEach-Object { $_.typing.latencyMs.p95 }); insertP95 = @($t2g | ForEach-Object { $_.insertMs.p95 })
  }
  search = $searchMid
  searchAll = @($search | ForEach-Object { [ordered]@{ totalMs = $_.totalMs; firstHitMs = $_.firstHitMs; matches = $_.matches } })
  txt300m = $t300Mid
  txt300mAll = @($t300 | ForEach-Object { [ordered]@{ firstPaintMs = $_.firstPaintMs; caretMax = $_.caretMs.max; wrapMax = $_.wrapToggleMs.max } })
  save = $saveMid
  saveAll = $saves
  launch = [ordered]@{ firstContentMs = $launch; p50 = (Median $launch { $_ }) }
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
Write-Host ("launch-2g first content (process creation → rows on screen): " + ($launch -join ", ") + " ms")
Write-Host "results: $Out"
if ($rows | Where-Object { -not $_.pass }) { exit 1 }
