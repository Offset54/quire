#Requires -Version 7
<#
.SYNOPSIS
  Builds the bench flavor of the desktop app and runs one in-app scenario.

.DESCRIPTION
  The bench flavor is the desktop app built with the `bench-fixtures` cargo
  feature and the frontend in `--mode bench`, into target/bench so it never
  overwrites the product build. The scenario writes its results as JSON and
  the app exits.

.EXAMPLE
  tools/bench/run-app-bench.ps1 -Scenario scroll-list
  tools/bench/run-app-bench.ps1 -Scenario scroll-list -NoBuild -Transport channel
#>
param(
  [Parameter(Mandatory)] [string] $Scenario,
  [string] $Out,
  # Logical window size; 1280x720 at 150 % is the reference machine's 1920x1080.
  [string] $Viewport = "1280x720",
  # "auto" (SharedBuffer on Windows) or "channel" (cross-platform fallback).
  [ValidateSet("auto", "channel")] [string] $Transport = "auto",
  [int] $TimeoutSeconds = 300,
  [switch] $NoBuild
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$exe = Join-Path $root "target/bench/release/quire.exe"

if (-not $NoBuild) {
  $config = Join-Path $env:TEMP "quire-bench-tauri-conf.json"
  '{"build":{"beforeBuildCommand":"pnpm build:bench"}}' | Set-Content $config
  $env:CARGO_TARGET_DIR = Join-Path $root "target/bench"
  Push-Location (Join-Path $root "apps/desktop")
  try {
    cargo tauri build --no-bundle --features bench-fixtures --config $config
    if ($LASTEXITCODE -ne 0) { throw "bench build failed" }
  } finally {
    Pop-Location
    Remove-Item Env:CARGO_TARGET_DIR
  }
}

if (-not $Out) {
  $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
  $Out = Join-Path $root ".cache/bench/$Scenario-$stamp.json"
}
New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
Remove-Item $Out -ErrorAction SilentlyContinue

$env:QUIRE_BENCH = $Scenario
$env:QUIRE_BENCH_OUT = $Out
$env:QUIRE_BENCH_VIEWPORT = $Viewport
if ($Transport -eq "channel") { $env:QUIRE_TRANSPORT = "channel" } else { Remove-Item Env:QUIRE_TRANSPORT -ErrorAction SilentlyContinue }
try {
  $p = Start-Process $exe -PassThru
  if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
    Stop-Process $p -Force
    throw "scenario $Scenario timed out after $TimeoutSeconds s"
  }
} finally {
  "QUIRE_BENCH", "QUIRE_BENCH_OUT", "QUIRE_BENCH_VIEWPORT", "QUIRE_TRANSPORT" |
    ForEach-Object { Remove-Item "Env:$_" -ErrorAction SilentlyContinue }
}
if (-not (Test-Path $Out)) { throw "no results written to $Out" }
$Out
