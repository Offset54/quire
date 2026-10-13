#Requires -Version 7
<#
.SYNOPSIS
  Fetches the CommonMark and GFM spec tests into .cache/corpora/markdown.

.DESCRIPTION
  The spec files are CC-BY-SA 4.0 (see README.md in this folder): they are
  test inputs, fetched at pinned revisions and checked by SHA-256, never
  committed (MASTER_PROMPT §10). The Markdown spec harness
  (crates/markdown/tests/spec.rs) reads them from there.
#>
$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$dir = Join-Path $root ".cache/corpora/markdown"
New-Item -ItemType Directory -Force $dir | Out-Null

$cmark = "https://raw.githubusercontent.com/commonmark/commonmark-spec/0.31.2"
$gfm = "https://raw.githubusercontent.com/github/cmark-gfm/27d942c8b0a62d192f616e5bf3578f4b6a89e180"
$files = @(
  @{ Name = "commonmark-0.31.2-spec.txt"; Url = "$cmark/spec.txt"; Sha = "257c41ad946f7a1414a499aca402a1aa8fdac3678532266611348c1cf54f4b80" },
  @{ Name = "gfm-spec.txt"; Url = "$gfm/test/spec.txt"; Sha = "7d8e5814befec287ac116786d81ff14e0adc9b13295b4494649e995408fd871c" },
  @{ Name = "gfm-extensions.txt"; Url = "$gfm/test/extensions.txt"; Sha = "a2a45e98be9fca95f564f927265a0f63beea6cae5369d1cf4bde44caa51b2a3a" }
)
foreach ($f in $files) {
  $path = Join-Path $dir $f.Name
  $ok = (Test-Path $path) -and ((Get-FileHash $path -Algorithm SHA256).Hash.ToLower() -eq $f.Sha)
  if (-not $ok) {
    Invoke-WebRequest -Uri $f.Url -OutFile $path
    $hash = (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
    if ($hash -ne $f.Sha) {
      Remove-Item $path
      throw "$($f.Name): SHA-256 $hash, expected $($f.Sha)"
    }
  }
  Write-Host "$($f.Name) ok"
}
