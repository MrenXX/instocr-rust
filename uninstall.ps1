$ErrorActionPreference = 'Stop'

$installDir = Join-Path $env:LOCALAPPDATA 'InstOCR'
$residentTarget = Join-Path $installDir 'instocr.exe'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$runName = 'InstOCR'
$shortcutPath = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\InstOCR.lnk'

Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and ($_.Path -ieq $residentTarget) } |
    ForEach-Object {
        Write-Host "Stopping running InstOCR process $($_.Id)..."
        Stop-Process -Id $_.Id -Force
        Wait-Process -Id $_.Id -ErrorAction SilentlyContinue
    }

if (Test-Path -Path $runKey) {
    Remove-ItemProperty -Path $runKey -Name $runName -ErrorAction SilentlyContinue
}

if (Test-Path -LiteralPath $shortcutPath) {
    Remove-Item -LiteralPath $shortcutPath -Force
}

if (Test-Path -LiteralPath $installDir) {
    Remove-Item -LiteralPath $installDir -Recurse -Force
}

Write-Host 'InstOCR uninstalled.'
