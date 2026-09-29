@echo off
rem RustLua: verify a WeChat Mini Game release bundle (static layout/quota + a Node
rem harness that fakes the wx host and runs real frames), then - for a thin first
rem package - serve its exported asset pack on the port baked into remote-assets.json
rem so WeChat DevTools can preview it.
rem
rem Repository-level entry point, kept generic on purpose: the bundle is auto-discovered
rem under examples\*\builds\wechat-minigame\ (or passed with -BundleRoot). It does not
rem name a game project, so it works for every project in this repository.
rem Only want the static server (no verification)? Use serve-wechat-pack.bat.
rem
rem Keep this file ASCII-only and WITHOUT a UTF-8 BOM (see plugins\webgl\test-webgl.bat).
rem
rem Usage: test-wechat-minigame.bat [extra args for scripts\test-bundle.ps1]
rem   test-wechat-minigame.bat -BundleRoot examples\game_jhyt\builds\wechat-minigame\launcher
rem   test-wechat-minigame.bat -Port 8899

setlocal EnableExtensions
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-bundle.ps1" -Platform wechat-minigame %*
set "CODE=%ERRORLEVEL%"
endlocal & exit /b %CODE%
