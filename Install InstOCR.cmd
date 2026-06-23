@echo off
powershell -NoProfile -Command "Get-ChildItem -LiteralPath \"%~dp0\" -Recurse -File | Unblock-File" >nul 2>&1
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
pause
