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
rem Usage: test-webgl.bat [extra args for scripts\test-bundle.ps1]
rem   test-webgl.bat -BundleRoot examples\game_jhyt\builds\webgl\jianghu_youtian -Port 8199

setlocal EnableExtensions
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0test-bundle.ps1" -Platform webgl %*
set "CODE=%ERRORLEVEL%"
endlocal & exit /b %CODE%
