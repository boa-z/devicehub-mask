param(
    [string]$Python = "$env:LOCALAPPDATA\Programs\Python\Python312\python.exe",
    [string]$Udid = ""
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Python)) {
    throw "Python 3.12 was not found at '$Python'. Install it with: winget install --id Python.Python.3.12 --exact"
}

$appleService = Get-Service -Name "Apple Mobile Device Service" -ErrorAction SilentlyContinue
if ($null -eq $appleService) {
    throw "Apple Mobile Device Service is not installed. Install or repair the desktop version of iTunes."
}
if ($appleService.Status -ne "Running") {
    throw "Apple Mobile Device Service is not running. Start it, then reconnect the iPhone over USB."
}

$runtimeDir = Join-Path $env:LOCALAPPDATA "devicehub-mask\pymobiledevice3"
$runtimePython = Join-Path $runtimeDir "Scripts\python.exe"
$pymobiledevice = Join-Path $runtimeDir "Scripts\pymobiledevice3.exe"

if (-not (Test-Path -LiteralPath $pymobiledevice)) {
    Write-Host "Creating the DeviceHub device-preparation runtime..."
    & $Python -m venv $runtimeDir
    if ($LASTEXITCODE -ne 0) { throw "Failed to create the Python runtime." }
    & $runtimePython -m pip install "pymobiledevice3==9.38.0"
    if ($LASTEXITCODE -ne 0) { throw "Failed to install pymobiledevice3 9.38.0." }
}

$devices = @(& $pymobiledevice usbmux list | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) { throw "Unable to query usbmuxd. Check the iTunes installation." }
$usbDevices = @($devices | Where-Object { $_.ConnectionType -eq "USB" })
if ($Udid) {
    $usbDevices = @($usbDevices | Where-Object { $_.Identifier -eq $Udid })
}
if ($usbDevices.Count -eq 0) {
    throw "No matching USB device was found. Connect and unlock the iPhone, then accept Trust This Computer."
}
if ($usbDevices.Count -gt 1 -and -not $Udid) {
    $ids = ($usbDevices.Identifier -join ", ")
    throw "Multiple USB devices were found ($ids). Run again with -Udid <identifier>."
}

$device = $usbDevices[0]
$Udid = $device.Identifier
Write-Host "Preparing $($device.DeviceName) ($Udid), iOS $($device.ProductVersion)..."

$developerMode = (& $pymobiledevice mounter query-developer-mode-status --udid $Udid).Trim()
if ($LASTEXITCODE -ne 0) { throw "Unable to query Developer Mode." }
if ($developerMode -ne "true") {
    throw "Developer Mode is disabled. Enable Settings > Privacy & Security > Developer Mode, reboot the iPhone, and run this script again."
}

$mountedImages = @(& $pymobiledevice mounter list --udid $Udid | ConvertFrom-Json)
if ($LASTEXITCODE -ne 0) { throw "Unable to query mounted developer images." }
if ($mountedImages.Count -eq 0) {
    Write-Host "Downloading and mounting the Personalized Developer Disk Image..."
    & $pymobiledevice mounter auto-mount --udid $Udid
    if ($LASTEXITCODE -ne 0) { throw "Personalized Developer Disk Image mounting failed." }
} else {
    Write-Host "Personalized Developer Disk Image is already mounted."
}

$previousUdid = $env:PYMOBILEDEVICE3_UDID
try {
    $env:PYMOBILEDEVICE3_UDID = $Udid
    $rsd = (& $pymobiledevice remote rsd-info --userspace | ConvertFrom-Json)
    if ($LASTEXITCODE -ne 0) { throw "Unable to query RSD over the USB CoreDeviceProxy tunnel." }
} finally {
    $env:PYMOBILEDEVICE3_UDID = $previousUdid
}

$displayService = $rsd.Services.PSObject.Properties.Name -contains "com.apple.coredevice.displayservice"
if (-not $displayService) {
    $serviceCount = @($rsd.Services.PSObject.Properties).Count
    throw "The DDI is mounted, but this device still advertises only $serviceCount RSD services and no displayservice. Reconnect USB and retry; if it persists, complete pairing once in Xcode 27 Device Hub."
}

Write-Host "USB displayservice is available. DeviceHub Mask is ready to start."
