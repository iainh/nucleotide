@echo off
setlocal

powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup-windows-runtime.ps1" %*
exit /b %ERRORLEVEL%
