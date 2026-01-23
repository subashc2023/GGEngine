@echo off
setlocal

set ROOT=%~dp0..

if exist "%ROOT%\\build" rmdir /s /q "%ROOT%\\build"
if exist "%ROOT%\\bin" rmdir /s /q "%ROOT%\\bin"

endlocal
