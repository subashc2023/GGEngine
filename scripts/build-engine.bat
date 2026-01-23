@echo off
setlocal

set ROOT=%~dp0..
call "%ROOT%\\scripts\\resolve-preset.bat" %1
if errorlevel 1 exit /b 1

cmake --preset %PRESET_CONFIG%
if errorlevel 1 exit /b 1

cmake --build --preset %BUILD_PRESET% --target Engine
if errorlevel 1 exit /b 1

endlocal
