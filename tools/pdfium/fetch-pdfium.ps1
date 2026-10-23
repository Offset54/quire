#Requires -Version 7
<#
.SYNOPSIS
  Fetches the pinned PDFium build (tools/pdfium/pdfium.json, ADR-0035) into
  .cache/pdfium/<version>/<platform> and checks it.

.DESCRIPTION
  Fails unless the archive's SHA-256 matches the manifest, its args.gn has the
  required settings (no V8, no XFA, release), and its licenses/ folder holds
  exactly the files the manifest lists, each with an approved licence (the
  native counterpart of cargo-deny and check-js-licenses, ADR-0019). Writes
  THIRD_PARTY_NOTICES.txt next to the library: every licence text, to ship
  with it. Prints the library's path last.

.PARAMETER Platform
  windows-x64, windows-arm64, macos-arm64, macos-x64 or linux-x64; defaults
  to the running machine.
#>
param([string]$Platform = "")
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/../..").Path
$manifest = Get-Content (Join-Path $PSScriptRoot "pdfium.json") -Raw | ConvertFrom-Json -AsHashtable

if (-not $Platform) {
  $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLower()
  $os = if ($IsWindows) { "windows" } elseif ($IsMacOS) { "macos" } else { "linux" }
  $Platform = "$os-$arch"
}
$asset = $manifest.assets[$Platform]
if (-not $asset) { throw "No pinned PDFium build for $Platform" }

$dir = Join-Path $root ".cache/pdfium/$($manifest.version)"
$out = Join-Path $dir $Platform
$tgz = Join-Path $dir $asset.file
New-Item -ItemType Directory -Force $dir | Out-Null

function Sha([string]$path) { (Get-FileHash $path -Algorithm SHA256).Hash.ToLower() }

if (-not (Test-Path $tgz) -or (Sha $tgz) -ne $asset.sha256) {
  $tag = [uri]::EscapeDataString($manifest.release)
  Invoke-WebRequest -Uri "$($manifest.source)/releases/download/$tag/$($asset.file)" -OutFile $tgz
  $hash = Sha $tgz
  if ($hash -ne $asset.sha256) {
    Remove-Item $tgz
    throw "$($asset.file): SHA-256 $hash, expected $($asset.sha256)"
  }
}

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
# Windows' own bsdtar: a GNU tar earlier on PATH (Git's) reads "C:" as a host.
$tar = if ($IsWindows) { Join-Path $env:SystemRoot "System32/tar.exe" } else { "tar" }
& $tar -xzf $tgz -C $out
if ($LASTEXITCODE -ne 0) { throw "tar failed on $tgz" }

# Build settings.
$gn = @{}
foreach ($line in Get-Content (Join-Path $out "args.gn")) {
  if ($line -match '^\s*(\w+)\s*=\s*(.+?)\s*$') { $gn[$Matches[1]] = $Matches[2] }
}
foreach ($k in $manifest.requiredArgs.Keys) {
  if ($gn[$k] -ne $manifest.requiredArgs[$k]) {
    throw "args.gn: $k = '$($gn[$k])', required '$($manifest.requiredArgs[$k])'"
  }
}

# Licences: exactly the reviewed set.
$found = @(Get-ChildItem (Join-Path $out "licenses") -File | ForEach-Object Name | Sort-Object)
$known = @($manifest.licenses.Keys | Sort-Object)
$unknown = @($found | Where-Object { $_ -notin $known })
$missing = @($known | Where-Object { $_ -notin $found })
if ($unknown) { throw "Unreviewed licence files in the PDFium build: $($unknown -join ', ') (add them to pdfium.json after review)" }
if ($missing) { throw "Licence files missing from the PDFium build: $($missing -join ', ')" }
foreach ($f in @("LICENSE")) {
  if (-not (Test-Path (Join-Path $out $f))) { throw "$f missing from the PDFium build" }
}

# Notices to ship next to the library.
$sb = [System.Text.StringBuilder]::new()
[void]$sb.AppendLine("PDFium $($manifest.version) ($($manifest.source), $($manifest.release))")
[void]$sb.AppendLine("Third-party notices for the PDFium library shipped with quire.")
foreach ($name in $known) {
  $l = $manifest.licenses[$name]
  [void]$sb.AppendLine("")
  [void]$sb.AppendLine(("=" * 78))
  [void]$sb.AppendLine("$name ($($l.license))")
  [void]$sb.AppendLine(("=" * 78))
  [void]$sb.AppendLine(((Get-Content (Join-Path $out "licenses/$name") -Raw) -replace "`r`n", "`n").TrimEnd())
}
[System.IO.File]::WriteAllText((Join-Path $out "THIRD_PARTY_NOTICES.txt"), ($sb.ToString() -replace "`r`n", "`n"))

$lib = Join-Path $out $asset.lib
if (-not (Test-Path $lib)) { throw "Library missing: $($asset.lib)" }
Write-Host "PDFium $($manifest.version) for $Platform checked: $($found.Count) licence files, all reviewed"
Write-Output $lib
