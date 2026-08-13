@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Flash-Firmware.ps1"
if errorlevel 1 (
  echo.
  echo Firmware flashing failed. See the message above.
  pause
)
