@echo off
rem RustLua: verify a WebGL release bundle and serve it on http://127.0.0.1:8137/
rem
rem Repository-level entry point, kept generic on purpose: the bundle is auto-discovered
rem under examples\*\builds\webgl\ (or passed with -BundleRoot). It does not name a game
rem project, so it works for every project in this repository. The double-click entry
rem that ships inside a release directory is a separate file: plugins\webgl\test-webgl.bat.
rem
rem Keep this file ASCII-only and WITHOUT a UTF-8 BOM (see plugins\webgl\test-webgl.bat).
rem
rem Usage: test-webgl.bat [extra args for scripts\test\test-bundle.ps1]
rem   test-webgl.bat -BundleRoot examples\game_jhyt\builds\webgl -Port 8199

setlocal EnableExtensions
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-bundle.ps1" -Platform webgl %*
set "CODE=%ERRORLEVEL%"
if not "%CODE%"=="0" call :hold_on_failure
endlocal & exit /b %CODE%

rem Hold a double-clicked console open after a failure (same contract as
rem plugins\webgl\test-webgl.bat): only when stdout is still the console, never
rem when output is redirected/piped into a tool.
:hold_on_failure
powershell -NoProfile -Command "if ([Console]::IsOutputRedirected) { exit 1 } else { exit 0 }"
if errorlevel 1 goto :eof
echo.
echo [RustLua] Failed. Press any key to close this window.
pause >nul
goto :eof
