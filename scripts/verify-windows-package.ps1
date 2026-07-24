param(
  [Parameter(Mandatory = $true)]
  [string]$TargetDirectory,
  [switch]$ApplicationOnly
)

$ErrorActionPreference = "Stop"

function Assert-GuiSubsystem([string]$Executable) {
  $bytes = [IO.File]::ReadAllBytes($Executable)
  if ($bytes.Length -lt 256 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) {
    throw "$Executable is not a valid PE executable"
  }
  $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
  $signature = [Text.Encoding]::ASCII.GetString($bytes, $peOffset, 4)
  if ($signature -ne "PE`0`0") { throw "$Executable has no PE signature" }
  $subsystem = [BitConverter]::ToUInt16($bytes, $peOffset + 24 + 68)
  if ($subsystem -ne 2) {
    throw "$Executable uses PE subsystem $subsystem; expected Windows GUI (2)"
  }
}

function Assert-PackagedRuntime([string]$Directory, [string]$PackageName) {
  $ffmpeg = Get-ChildItem $Directory -Recurse -File -Filter "ffmpeg.exe" | Select-Object -First 1
  $license = Get-ChildItem $Directory -Recurse -File -Filter "ffmpeg-LICENSE.txt" | Select-Object -First 1
  if (-not $ffmpeg) { throw "$PackageName does not contain ffmpeg.exe" }
  if (-not $license) { throw "$PackageName does not contain ffmpeg-LICENSE.txt" }
  & $ffmpeg.FullName -hide_banner -version
  if ($LASTEXITCODE -ne 0) { throw "FFmpeg extracted from $PackageName is not executable" }
}

function Get-MsiFileNames([string]$Package) {
  $installer = New-Object -ComObject WindowsInstaller.Installer
  $database = $null
  $view = $null
  $record = $null
  try {
    $invoke = [Reflection.BindingFlags]::InvokeMethod
    $getProperty = [Reflection.BindingFlags]::GetProperty
    $database = $installer.GetType().InvokeMember(
      "OpenDatabase", $invoke, $null, $installer, @($Package, 0)
    )
    $view = $database.GetType().InvokeMember(
      "OpenView", $invoke, $null, $database, @("SELECT ``FileName`` FROM ``File``")
    )
    $view.GetType().InvokeMember("Execute", $invoke, $null, $view, $null) | Out-Null

    $files = @()
    while ($true) {
      $record = $view.GetType().InvokeMember("Fetch", $invoke, $null, $view, $null)
      if (-not $record) { break }
      $files += $record.GetType().InvokeMember(
        "StringData", $getProperty, $null, $record, @(1)
      )
      [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null
      $record = $null
    }
    return $files
  } finally {
    if ($record) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($record) | Out-Null }
    if ($view) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($view) | Out-Null }
    if ($database) { [Runtime.InteropServices.Marshal]::FinalReleaseComObject($database) | Out-Null }
    [Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer) | Out-Null
  }
}

function Assert-MsiRuntime([string]$Package) {
  $files = Get-MsiFileNames $Package
  if (-not ($files | Where-Object { $_ -match '(^|\|)ffmpeg\.exe$' })) {
    throw "MSI does not contain ffmpeg.exe"
  }
  if (-not ($files | Where-Object { $_ -match '(^|\|)ffmpeg-LICENSE\.txt$' })) {
    throw "MSI does not contain ffmpeg-LICENSE.txt"
  }
}

$application = Join-Path $TargetDirectory "devicehub-mask.exe"
if (-not (Test-Path $application -PathType Leaf)) { throw "Built application is missing: $application" }
Assert-GuiSubsystem $application
if ($ApplicationOnly) {
  Write-Host "Verified Windows GUI subsystem."
  exit 0
}

$bundleDirectory = Join-Path $TargetDirectory "bundle"
$msi = Get-ChildItem $bundleDirectory -Recurse -File -Filter "*.msi" | Select-Object -First 1
$nsis = Get-ChildItem $bundleDirectory -Recurse -File -Filter "*-setup.exe" | Select-Object -First 1
if (-not $msi -or -not $nsis) { throw "Tauri did not produce both MSI and NSIS packages" }

$extractRoot = Join-Path $env:RUNNER_TEMP "devicehub-mask-package-verification"
Remove-Item $extractRoot -Recurse -Force -ErrorAction SilentlyContinue
$nsisRoot = Join-Path $extractRoot "nsis"
New-Item $nsisRoot -ItemType Directory -Force | Out-Null

Assert-MsiRuntime $msi.FullName

& 7z.exe x "-o$nsisRoot" $nsis.FullName -y | Out-Null
if ($LASTEXITCODE -ne 0) { throw "Unable to extract NSIS package (exit $LASTEXITCODE)" }
$nestedArchives = Get-ChildItem $nsisRoot -Recurse -File -Filter "*.7z"
foreach ($archive in $nestedArchives) {
  $archiveRoot = Join-Path $archive.DirectoryName $archive.BaseName
  New-Item $archiveRoot -ItemType Directory -Force | Out-Null
  & 7z.exe x "-o$archiveRoot" $archive.FullName -y | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "Unable to extract nested NSIS payload $($archive.Name)" }
}
Assert-PackagedRuntime $nsisRoot "NSIS"

Write-Host "Verified GUI subsystem and bundled FFmpeg in MSI and NSIS packages."
