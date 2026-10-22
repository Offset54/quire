#Requires -Version 7
# Fetches the PDFium build the M3 engine spike measured: bblanchon/pdfium-binaries
# chromium/7881 (PDFium 151.0.7881.0), Windows x64, no V8 or XFA, into
# .cache/pdfium/7881/x (ignored by git). The archive's SHA-256 is pinned.
#
#   pwsh spikes/pdf-engine/fetch-pdfium.ps1
#   target/release/pdf-engine.exe pdfium .cache/pdfium/7881/x/bin/pdfium.dll <file.pdf>

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '../..')
$dir = Join-Path $root '.cache/pdfium/7881'
$tgz = Join-Path $dir 'pdfium-win-x64.tgz'
$url = 'https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-win-x64.tgz'
$sha = '73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08'

New-Item -ItemType Directory -Force $dir | Out-Null
if (-not (Test-Path $tgz) -or (Get-FileHash $tgz -Algorithm SHA256).Hash -ne $sha) {
    Invoke-WebRequest $url -OutFile $tgz
}
$got = (Get-FileHash $tgz -Algorithm SHA256).Hash
if ($got -ne $sha) { throw "SHA-256 mismatch: $got" }
$out = Join-Path $dir 'x'
New-Item -ItemType Directory -Force $out | Out-Null
tar -xzf $tgz -C $out
Write-Host "PDFium in $out/bin/pdfium.dll"
