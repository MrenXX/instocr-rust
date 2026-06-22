param(
    [switch]$NoStart
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$dist = Join-Path $root 'dist\windows-x64'
$installDir = Join-Path $env:LOCALAPPDATA 'InstOCR'
$residentName = 'instocr.exe'
$cliName = 'instocr-cli.exe'
$residentSource = Join-Path $dist $residentName
$cliSource = Join-Path $dist $cliName
$residentTarget = Join-Path $installDir $residentName
$cliTarget = Join-Path $installDir $cliName
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$runName = 'InstOCR'

function Stop-InstalledInstOCR {
    Get-Process -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -ieq $residentTarget) } |
        ForEach-Object {
            Write-Host "Stopping running InstOCR process $($_.Id)..."
            Stop-Process -Id $_.Id -Force
            Wait-Process -Id $_.Id -ErrorAction SilentlyContinue
        }
}

if (!(Test-Path -LiteralPath $residentSource) -or !(Test-Path -LiteralPath $cliSource)) {
    Write-Host 'Prebuilt binaries not found; building with cargo...'
    Push-Location $root
    try {
        cargo build --release
    }
    finally {
        Pop-Location
    }
    $residentSource = Join-Path $root 'target\release\instocr.exe'
    $cliSource = Join-Path $root 'target\release\instocr-cli.exe'
}

if (!(Test-Path -LiteralPath $residentSource) -or !(Test-Path -LiteralPath $cliSource)) {
    throw 'Could not find or build InstOCR binaries.'
}

New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Stop-InstalledInstOCR

Copy-Item -LiteralPath $residentSource -Destination $residentTarget -Force
Copy-Item -LiteralPath $cliSource -Destination $cliTarget -Force

Copy-Item -LiteralPath (Join-Path $root 'configure-hotkeys.ps1') -Destination (Join-Path $installDir 'configure-hotkeys.ps1') -Force
Copy-Item -LiteralPath (Join-Path $root 'Configure Hotkeys.cmd') -Destination (Join-Path $installDir 'Configure Hotkeys.cmd') -Force

New-Item -Path $runKey -Force | Out-Null
Set-ItemProperty -Path $runKey -Name $runName -Value "`"$residentTarget`""

$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$shortcutPath = Join-Path $startMenu 'InstOCR.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $residentTarget
$shortcut.WorkingDirectory = $installDir
$shortcut.Description = 'InstOCR lightweight OCR capture'
$shortcut.Save()

if (!$NoStart) {
    Start-Process -FilePath $residentTarget
}

Write-Host ''
Write-Host 'InstOCR installed.'
Write-Host "Install path: $installDir"
Write-Host 'Startup at login: enabled'
Write-Host 'Capture hotkey: Ctrl+Alt+D'
Write-Host 'Cycle OCR language: Ctrl+Alt+W'
Write-Host 'Use the tray icon to exit.'
