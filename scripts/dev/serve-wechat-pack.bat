@echo off
rem RustLua: serve the WeChat Mini Game **asset pack** so DevTools can preview a thin first package.
rem
rem Equivalent to:
rem   powershell -NoProfile -File scripts\dev\serve-pack.ps1 ^
rem     -Root examples\game_jhyt\builds\wechat-minigame\assets -Port 8787
rem but with no project name and no port hardcoded: the bundle is auto-discovered under
rem examples\*\builds\wechat-minigame\, and the port comes from the bundle's remote-assets.json
rem (serving on another port would make DevTools fetch a dead address).
rem
rem It only starts the static server (no verification) - use test-wechat-minigame.bat when you
rem also want the platform verifier to run first.
rem
rem Keep this file ASCII-only and WITHOUT a UTF-8 BOM (see plugins\webgl\test-webgl.bat).
rem
rem Usage: serve-wechat-pack.bat [extra args for scripts\test\test-bundle.ps1]
rem   serve-wechat-pack.bat -Port 8899
rem   serve-wechat-pack.bat -BundleRoot examples\game_jhyt\builds\wechat-minigame\launcher

setlocal EnableExtensions
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0..\test\test-bundle.ps1" -Platform wechat-minigame -NoVerify %*
set "CODE=%ERRORLEVEL%"
endlocal & exit /b %CODE%
