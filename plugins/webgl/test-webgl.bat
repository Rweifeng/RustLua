@echo off
rem RustLua WebGL bundle: verify the release and serve it on http://127.0.0.1:8137/
rem
rem This file is a PLATFORM PLUGIN TEMPLATE. It is copied into every WebGL release
rem directory by scripts\package.ps1 (it must be: release-atomic.ps1 moves everything
rem that is not .staging-*/.backup-* into a backup and deletes it, so a hand-dropped
rem copy would vanish on the next publish).
rem
rem It does NOT hardcode a game project name or a directory depth. Instead it walks
rem up from its own location until it finds the repository root (the directory that
rem contains scripts\test-bundle.ps1). That makes the same file work for any game
rem project, any publish_name, and any bundle location inside the repository.
rem
rem Keep this file ASCII-only and WITHOUT a UTF-8 BOM: a BOM before @echo off prints
rem garbage, and non-ASCII bytes mojibake under the console's default code page.
rem
rem Exit codes (same contract as scripts\test-bundle.ps1):
rem   0 = verified and served   1 = verification failed   2 = environment limited
rem   3 = repository root not found (bundle copied outside the repository)

setlocal EnableExtensions

rem %~dp0 always ends with a backslash; strip it so probes do not build "dir\\scripts".
set "BUNDLE=%~dp0"
if "%BUNDLE:~-1%"=="\" set "BUNDLE=%BUNDLE:~0,-1%"

rem Optional first argument: port (digits only), default 8137.
set "PORT=8137"
echo %~1|findstr /r "^[0-9][0-9]*$" >nul && set "PORT=%~1"

rem Is this file running from inside a release directory, or from the plugin source
rem tree (a developer checkout)? A release directory always carries the bundle files;
rem plugins\webgl does not. Without this check, running the template in place would
rem treat plugins\webgl as a bundle and fail with a misleading "not a valid bundle".
set "IS_BUNDLE=1"
if not exist "%BUNDLE%\index.html" set "IS_BUNDLE="
if not exist "%BUNDLE%\web-runtime.js" set "IS_BUNDLE="
if not exist "%BUNDLE%\pkg\game_web.js" set "IS_BUNDLE="
if not exist "%BUNDLE%\pkg\game_web_bg.wasm" set "IS_BUNDLE="
if not exist "%BUNDLE%\pkg\fengari-web.js" set "IS_BUNDLE="

rem Walk up until the repository root shows up. %~dp0 is the only reliable anchor:
rem on double-click the current directory is %SystemRoot%\System32, not the bundle.
rem %%I in ("%PROBE%\..") with %%~fI fully qualifies and normalises the "..", so the
rem probe string never grows; no pushd/popd and no delayed expansion are needed.
set "ROOT="
set "PROBE=%BUNDLE%"
:find_root
if "%PROBE%"=="" goto no_root
if exist "%PROBE%\scripts\test-bundle.ps1" set "ROOT=%PROBE%" & goto found_root
for %%I in ("%PROBE%\..") do set "NEXT=%%~fI"
if /i "%NEXT%"=="%PROBE%" goto no_root
set "PROBE=%NEXT%"
goto find_root

:found_root
echo [RustLua] repository root: %ROOT%
if not defined IS_BUNDLE goto discover

echo [RustLua] bundle: %BUNDLE%  port: %PORT%
powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\scripts\test-bundle.ps1" -Platform webgl -BundleRoot "%BUNDLE%" -Port %PORT%
goto report

:discover
echo [RustLua] %BUNDLE%
echo           is not a release directory (no index.html / pkg\\* - this is the plugin
echo           source tree, not a copy of a WebGL release). Auto-discovering one instead.
powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\scripts\test-bundle.ps1" -Platform webgl -Port %PORT%

:report
set "CODE=%ERRORLEVEL%"
if "%CODE%"=="2" echo [RustLua] Environment limited (no node or no browser) - this result is NOT a pass.
if not "%CODE%"=="0" echo [RustLua] test-bundle.ps1 exited %CODE%. Logs: artifacts\logs\webgl.
endlocal & exit /b %CODE%

:no_root
echo [RustLua] Could not find scripts\test-bundle.ps1 in any parent of:
echo           %BUNDLE%
echo [RustLua] This bundle is outside the repository, so it cannot be verified here.
echo [RustLua] Serve it manually instead (see run.txt):
echo   python -m http.server %PORT% --directory "%BUNDLE%"
exit /b 3
