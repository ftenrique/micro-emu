@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Uninstall-Codex-Micro.ps1"
if errorlevel 1 (
  echo.
  echo Uninstall failed. See the message above.
  pause
)
