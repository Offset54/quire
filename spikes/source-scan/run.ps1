#Requires -Version 7
# Runs the ADR-0007 scan spike cold (file purged before each run) and warm.
param([string] $File = "$PSScriptRoot/../../.cache/fixtures/mixed-lines-2gib.txt", [int] $Runs = 3)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$refsim = Join-Path $root "target/release/refsim.exe"
$exe = Join-Path $root "target/release/source-scan.exe"
$results = @()
foreach ($variant in "pread", "mmap", "mmap-prefetch") {
  foreach ($threads in 1, 4) {
    foreach ($i in 1..$Runs) {
      & $refsim purge $File | Out-Null
      $r = & $refsim run --pcores --logical 8 -- $exe $File $variant $threads
      $results += ($r | ConvertFrom-Json | Add-Member -PassThru cache cold)
    }
    $r = & $refsim run --pcores --logical 8 -- $exe $File $variant $threads
    $results += ($r | ConvertFrom-Json | Add-Member -PassThru cache warm)
  }
}
$results | Format-Table variant, threads, cache, seconds, mbPerSecond, lines -AutoSize | Out-String -Width 160 | Write-Host
$results | ConvertTo-Json -Depth 4
