@echo off
setlocal

set ROOT=%~dp0..

call "%ROOT%\\scripts\\clean.bat" %1
if errorlevel 1 exit /b 1

call "%ROOT%\\scripts\\build-all.bat" %1
if errorlevel 1 exit /b 1

endlocal
