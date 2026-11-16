@echo off
setlocal
"%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "%~dp0manage-dedup.ps1"
set "DEDUP_EXIT=%ERRORLEVEL%"
if not "%DEDUP_EXIT%"=="0" pause
exit /b %DEDUP_EXIT%