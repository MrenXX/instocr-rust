@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0configure-hotkeys.ps1" %*
pause
