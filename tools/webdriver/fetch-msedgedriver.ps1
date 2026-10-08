#Requires -Version 7
<#
.SYNOPSIS
  Downloads the msedgedriver that matches the installed WebView2 runtime
  (they must match exactly) into .cache/webdriver/<version>/, and prints
  the driver's path.
#>
$ErrorActionPreference = "Stop"
$root = (Resolve-Path "$PSScriptRoot/../..").Path
$key = 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
$version = (Get-ItemProperty $key -ErrorAction SilentlyContinue).pv
if (-not $version) {
  $version = (Get-ItemProperty 'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue).pv
}
if (-not $version) { throw "WebView2 runtime not found" }
$dir = Join-Path $root ".cache/webdriver/$version"
$exe = Join-Path $dir "msedgedriver.exe"
if (-not (Test-Path $exe)) {
  New-Item -ItemType Directory -Force $dir | Out-Null
  $zip = Join-Path $dir "edgedriver_win64.zip"
  $url = "https://msedgedriver.microsoft.com/$version/edgedriver_win64.zip"
  Write-Host "downloading $url"
  Invoke-WebRequest $url -OutFile $zip
  Expand-Archive $zip -DestinationPath $dir -Force
  Remove-Item $zip
}
$exe
