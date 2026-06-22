$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$installDir = Join-Path $env:LOCALAPPDATA 'InstOCR'
$cli = Join-Path $installDir 'instocr-cli.exe'
if (!(Test-Path -LiteralPath $cli)) {
    $cli = Join-Path $scriptDir 'dist\windows-x64\instocr-cli.exe'
}
if (!(Test-Path -LiteralPath $cli)) {
    $cli = Join-Path $scriptDir 'target\release\instocr-cli.exe'
}
if (!(Test-Path -LiteralPath $cli)) {
    throw 'Could not find instocr-cli.exe. Install or build InstOCR first.'
}

$settingsDir = Join-Path $env:APPDATA 'InstOCR'
$settingsPath = Join-Path $settingsDir 'rust-settings.txt'
New-Item -ItemType Directory -Force -Path $settingsDir | Out-Null

function Validate-Hotkey($Prompt, $Default) {
    while ($true) {
        $value = Read-Host "$Prompt [$Default]"
        if ([string]::IsNullOrWhiteSpace($value)) {
            $value = $Default
        }

        $output = & $cli --validate-hotkey $value 2>&1
        if ($LASTEXITCODE -eq 0) {
            return ($output | Select-Object -First 1).ToString().Trim()
        }

        Write-Host "Invalid hotkey: $output" -ForegroundColor Yellow
    }
}

$capture = Validate-Hotkey 'Capture hotkey' 'Ctrl+Alt+D'
$cycle = Validate-Hotkey 'Cycle OCR language hotkey' 'Ctrl+Alt+W'

if ($capture -ieq $cycle) {
    throw 'Capture and cycle hotkeys cannot be the same.'
}

$existing = @{}
if (Test-Path -LiteralPath $settingsPath) {
    foreach ($line in Get-Content -LiteralPath $settingsPath) {
        $parts = $line.Split('=', 2)
        if ($parts.Count -eq 2) {
            $existing[$parts[0]] = $parts[1]
        }
    }
}

$existing['capture_hotkey'] = $capture
$existing['cycle_hotkey'] = $cycle

$lines = @(
    "language_tag=$($existing['language_tag'])",
    "capture_hotkey=$capture",
    "cycle_hotkey=$cycle"
)

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText($settingsPath, ($lines -join [Environment]::NewLine) + [Environment]::NewLine, $utf8NoBom)

Write-Host ''
Write-Host "Saved hotkeys to $settingsPath"
Write-Host "Capture: $capture"
Write-Host "Cycle OCR language: $cycle"
Write-Host 'Restart InstOCR from the tray for changes to take effect.'
