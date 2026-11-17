@echo off
setlocal
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\build-installer.ps1"
set "result=%ERRORLEVEL%"
if not "%result%"=="0" echo Installer build failed with exit code %result%.
pause
exit /b %result%
