@echo off
rem RustLua WebGL bundle: verify the release and serve it on http://127.0.0.1:8137/
rem
rem Usage:  test-webgl.bat [port] [host] [tls]
rem   [port]  digits only, default 8137. If that port is busy the verifier falls
rem           back to the next free one (8138..8147) and prints the real URL.
rem   [host]  listen address, default 0.0.0.0 (= all interfaces). A phone/tablet on the
rem           same Wi-Fi can then open the LAN URL printed below; nobody has to guess it
rem           from ipconfig. Pass 127.0.0.1 to keep it loopback-only.
rem   [tls]   pass "tls" to serve over HTTPS with a self-signed dev certificate. Only a
rem           secure context makes the browser advertise "br": the wasm is 2.07 MB over
rem           HTTP-gzip vs 1.76 MB over HTTPS-brotli. The phone warns once - tap
rem           Advanced / Proceed.
rem
rem Why 0.0.0.0 is the default here (and 127.0.0.1 stays the default for the gates):
rem this file's whole job is to open a local playtest entry, and the first thing a
rem playtester does is try it on their phone. Defaulting to loopback printed a
rem 127.0.0.1 URL that the phone could not open - the pin was the surprise, not the
rem exposure. Gate scripts (verify-*/test-bundle.ps1 without -BindHost) still bind loopback.
rem
rem This file is a PLATFORM PLUGIN TEMPLATE. It is copied into every WebGL release
rem directory by scripts\build\package.ps1 (it must be: release-atomic.ps1 moves everything
rem that is not .staging-*/.backup-* into a backup and deletes it, so a hand-dropped
rem copy would vanish on the next publish).
rem
rem It does NOT hardcode a game project name or a directory depth. Instead it walks
rem up from its own location until it finds the repository root (the directory that
rem contains scripts\test\test-bundle.ps1). That makes the same file work for any game
rem project, any publish_name, and any bundle location inside the repository.
rem
rem Keep this file ASCII-only and WITHOUT a UTF-8 BOM: a BOM before @echo off prints
rem garbage, and non-ASCII bytes mojibake under the console's default code page.
rem
rem Exit codes (same contract as scripts\test\test-bundle.ps1):
rem   0 = verified and served   1 = verification failed   2 = environment limited
rem   3 = repository root not found (bundle copied outside the repository)

setlocal EnableExtensions

rem %~dp0 always ends with a backslash; strip it so probes do not build "dir\\scripts".
set "BUNDLE=%~dp0"
if "%BUNDLE:~-1%"=="\" set "BUNDLE=%BUNDLE:~0,-1%"

rem Optional first argument: port (digits only), default 8137.
set "PORT=8137"
echo %~1|findstr /r "^[0-9][0-9]*$" >nul && set "PORT=%~1"

rem Optional second argument: listen address, default 0.0.0.0 (all interfaces).
rem Pass 127.0.0.1 explicitly to keep it loopback-only.
set "BINDHOST=0.0.0.0"
if not "%~2"=="" set "BINDHOST=%~2"

rem Optional third argument: "tls" serves HTTPS (unlocks brotli in the browser).
set "TLSSWITCH="
if /i "%~3"=="tls" set "TLSSWITCH=-Tls"

rem Is this file running from inside a release directory, or from the plugin source
rem tree (a developer checkout)? A release directory always carries the bundle files;
rem plugins\webgl does not. Without this check, running the template in place would
rem treat plugins\webgl as a bundle and fail with a misleading "not a valid bundle".
rem NOTE: web-runtime.js used to be one of these markers. It was removed with the old
rem JS world (B8), so keeping it here made every real release directory look like "not
rem a bundle" and the script silently fell into auto-discovery instead.
set "IS_BUNDLE=1"
if not exist "%BUNDLE%\index.html" set "IS_BUNDLE="
if not exist "%BUNDLE%\pkg\game_web.js" set "IS_BUNDLE="
if not exist "%BUNDLE%\pkg\game_web_bg.wasm" set "IS_BUNDLE="

rem Walk up until the repository root shows up. %~dp0 is the only reliable anchor:
rem on double-click the current directory is %SystemRoot%\System32, not the bundle.
rem %%I in ("%PROBE%\..") with %%~fI fully qualifies and normalises the "..", so the
rem probe string never grows; no pushd/popd and no delayed expansion are needed.
set "ROOT="
set "PROBE=%BUNDLE%"
:find_root
if "%PROBE%"=="" goto no_root
if exist "%PROBE%\scripts\test\test-bundle.ps1" set "ROOT=%PROBE%" & goto found_root
for %%I in ("%PROBE%\..") do set "NEXT=%%~fI"
if /i "%NEXT%"=="%PROBE%" goto no_root
set "PROBE=%NEXT%"
goto find_root

:found_root
echo [RustLua] repository root: %ROOT%
if not defined IS_BUNDLE goto discover

echo [RustLua] bundle: %BUNDLE%  port: %PORT%  listen: %BINDHOST%
if /i "%BINDHOST%"=="127.0.0.1" goto serve_loopback
echo [RustLua] Listening on all interfaces: the LAN URL to open on the phone is printed below.
powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\scripts\test\test-bundle.ps1" -Platform webgl -BundleRoot "%BUNDLE%" -Port %PORT% -BindHost %BINDHOST% %TLSSWITCH%
goto report

:serve_loopback
echo [RustLua] This binds loopback only - a phone on the same Wi-Fi cannot open it.
echo [RustLua] For a real device run: test-webgl.bat %PORT% 0.0.0.0
powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\scripts\test\test-bundle.ps1" -Platform webgl -BundleRoot "%BUNDLE%" -Port %PORT%
goto report

:discover
echo [RustLua] %BUNDLE%
echo           is not a release directory (no index.html / pkg\\* - this is the plugin
echo           source tree, not a copy of a WebGL release). Auto-discovering one instead.
echo [RustLua] listen: %BINDHOST%  (pass 127.0.0.1 as the 2nd argument to keep it loopback-only)
powershell -NoProfile -ExecutionPolicy Bypass -File "%ROOT%\scripts\test\test-bundle.ps1" -Platform webgl -Port %PORT% -BindHost %BINDHOST% %TLSSWITCH%

:report
set "CODE=%ERRORLEVEL%"
if "%CODE%"=="2" echo [RustLua] Environment limited (no node or no browser) - this result is NOT a pass.
if not "%CODE%"=="0" echo [RustLua] test-bundle.ps1 exited %CODE%. Logs: artifacts\logs\webgl.
if not "%CODE%"=="0" call :hold_on_failure
endlocal & exit /b %CODE%

:no_root
echo [RustLua] Could not find scripts\test\test-bundle.ps1 in any parent of:
echo           %BUNDLE%
echo [RustLua] This bundle is outside the repository, so it cannot be verified here.
echo [RustLua] Serve it manually instead (see run.txt):
echo   python -m http.server %PORT% --directory "%BUNDLE%"
call :hold_on_failure
exit /b 3

rem ---------------------------------------------------------------------------
rem Hold a double-clicked console open after a failure. A double-click closes the
rem window the moment the script exits, so "port busy" / "verification failed"
rem would vanish before anyone could read it. Only hold when stdout is still the
rem console: automation redirects or pipes the output and must never block on a
rem keypress (a gate that waits for a key is a hung gate).
:hold_on_failure
powershell -NoProfile -Command "if ([Console]::IsOutputRedirected) { exit 1 } else { exit 0 }"
if errorlevel 1 goto :eof
echo.
echo [RustLua] Failed. Press any key to close this window.
pause >nul
goto :eof
