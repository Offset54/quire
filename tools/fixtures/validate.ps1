#Requires -Version 7
<#
.SYNOPSIS
  Validates generated fixtures with the tools that matter (M0 task 11).

.DESCRIPTION
  - PDFs: `qpdf --check`, page counts, and linearization of the linearized copy.
  - DOCX: Word (hidden, via COM) opens it read-only without repair and paginates it.
  - PPTX: PowerPoint (windowless, via COM) opens it read-only; slides are counted.
  Office checks are skipped if Word or PowerPoint is already running, so this
  never touches the user's open documents. Office processes started here are
  killed if they hang. Results are written into the manifest under "validation".
#>
param(
  [string] $Dir = (Join-Path (Resolve-Path "$PSScriptRoot/../..") ".cache/fixtures"),
  [int] $OfficeTimeoutSeconds = 600
)
$ErrorActionPreference = "Stop"
$manifestPath = Join-Path $Dir "manifest.json"
$manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
$qpdf = (Get-Command qpdf -ErrorAction SilentlyContinue)?.Source ??
  (Get-ChildItem "C:\Program Files\qpdf*\bin\qpdf.exe" -ErrorAction SilentlyContinue | Select-Object -First 1)?.FullName
$results = [ordered]@{ date = (Get-Date).ToString("o") }

function Check-Pdf([string] $file, [int] $expectedPages, [switch] $Linearized) {
  $path = Join-Path $Dir $file
  if (-not (Test-Path $path)) { return @{ ok = $false; error = "missing" } }
  if (-not $qpdf) { return @{ ok = $false; error = "qpdf not found" } }
  $check = & $qpdf --check $path 2>&1 | Out-String
  $checkOk = $LASTEXITCODE -eq 0
  $pages = [int](& $qpdf --show-npages $path)
  $r = [ordered]@{ ok = $checkOk -and $pages -eq $expectedPages; qpdfCheck = $checkOk; pages = $pages }
  if (-not $checkOk) { $r.output = $check.Trim() }
  if ($Linearized) {
    $lin = & $qpdf --check-linearization $path 2>&1 | Out-String
    $r.linearization = $LASTEXITCODE -eq 0
    $r.ok = $r.ok -and $r.linearization
    if (-not $r.linearization) { $r.linearizationOutput = $lin.Trim() }
  }
  $r
}

<# Runs an Office COM check in a job; kills Office processes the job started if it hangs. #>
function Invoke-OfficeCheck([string] $processName, [scriptblock] $check, [string] $path) {
  if (Get-Process $processName -ErrorAction SilentlyContinue) {
    return @{ ok = $null; skipped = "$processName is already running; not touching the user's session" }
  }
  $started = Get-Date
  # Office automation instances can linger after Quit(); only windowless
  # instances started by this check are ever stopped.
  $cleanup = {
    Get-Process $processName -ErrorAction SilentlyContinue |
      Where-Object { $_.StartTime -ge $started -and $_.MainWindowHandle -eq 0 } |
      Stop-Process -Force
  }
  $job = Start-Job -ScriptBlock $check -ArgumentList $path
  try {
    if (Wait-Job $job -Timeout $OfficeTimeoutSeconds) {
      $out = Receive-Job $job -ErrorAction SilentlyContinue -ErrorVariable err
      if ($err) { return @{ ok = $false; error = ($err | Out-String).Trim() } }
      return $out | Select-Object * -ExcludeProperty PSComputerName, RunspaceId, PSShowComputerName
    }
    Stop-Job $job
    return @{ ok = $false; error = "timed out after $OfficeTimeoutSeconds s" }
  } finally {
    Remove-Job $job -Force
    Start-Sleep -Milliseconds 500
    & $cleanup
  }
}

$wordCheck = {
  param($path)
  $word = New-Object -ComObject Word.Application
  try {
    $word.Visible = $false
    $word.DisplayAlerts = 0
    # Open(FileName, ConfirmConversions, ReadOnly, AddToRecentFiles, PasswordDocument,
    #      PasswordTemplate, Revert, WritePasswordDocument, WritePasswordTemplate,
    #      Format, Encoding, Visible, OpenAndRepair, DocumentDirection, NoEncodingDialog)
    $doc = $word.Documents.Open($path, $false, $true, $false, "", "", $false, "", "", 0, 0, $false, $false, 0, $true)
    $doc.Repaginate()
    $pages = $doc.ComputeStatistics(2)   # wdStatisticPages
    $tables = $doc.Tables.Count
    $images = $doc.InlineShapes.Count
    $doc.Close(0)
    @{ ok = $true; pages = $pages; tables = $tables; inlineImages = $images; app = "Word $($word.Version)" }
  } finally {
    $word.Quit(0)
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($word)
  }
}

$pptCheck = {
  param($path)
  $ppt = New-Object -ComObject PowerPoint.Application
  try {
    # Open(FileName, ReadOnly, Untitled, WithWindow): msoTrue = -1, msoFalse = 0
    $pres = $ppt.Presentations.Open($path, -1, 0, 0)
    $slides = $pres.Slides.Count
    $pictures = 0
    foreach ($s in @($pres.Slides.Item(1), $pres.Slides.Item($slides))) {
      foreach ($shape in $s.Shapes) { if ($shape.Type -eq 13) { $pictures++ } }   # msoPicture
    }
    $pres.Close()
    @{ ok = $true; slides = $slides; picturesOnFirstAndLast = $pictures; app = "PowerPoint $($ppt.Version)" }
  } finally {
    $ppt.Quit()
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($ppt)
  }
}

$f = $manifest.fixtures
if ($f."pdf-5000p") {
  $results["pdf-5000p"] = Check-Pdf $f."pdf-5000p".file 5000
  $lin = $f."pdf-5000p".details.linearized.file
  if ($lin) { $results["pdf-5000p-linearized"] = Check-Pdf $lin 5000 -Linearized }
}
if ($f."pdf-scanned-1000p") { $results["pdf-scanned-1000p"] = Check-Pdf $f."pdf-scanned-1000p".file 1000 }
if ($f."docx-1000p") {
  $r = Invoke-OfficeCheck "WINWORD" $wordCheck (Join-Path (Resolve-Path $Dir) $f."docx-1000p".file)
  if ($r.ok) { $r.ok = $r.pages -eq 1000 }
  $results["docx-1000p"] = $r
}
if ($f."pptx-500s") {
  $r = Invoke-OfficeCheck "POWERPNT" $pptCheck (Join-Path (Resolve-Path $Dir) $f."pptx-500s".file)
  if ($r.ok) { $r.ok = $r.slides -eq 500 }
  $results["pptx-500s"] = $r
}

$manifest | Add-Member -NotePropertyName validation -NotePropertyValue $results -Force
$manifest | ConvertTo-Json -Depth 10 | Set-Content $manifestPath
$results | ConvertTo-Json -Depth 5
