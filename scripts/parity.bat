@echo off
setlocal

rem Differential parity battery: run every query through both the native
rem indexer and the Everything engine, and diff the counts. Any DIFF line is
rem a behavioral mismatch - a bug in the native engine (Everything is the
rem reference implementation).
rem
rem Requires:
rem   - The instant-file-search-indexer service RUNNING (native engine up)
rem   - The Everything engine reachable (bundled or installed)
rem
rem Run from anywhere; resolves the repo relative to this file.

set "REPO=%~dp0.."
cd /d "%REPO%"

echo [check] Native indexer service status...
sc.exe query instant-file-search-indexer | findstr /C:"RUNNING" >nul
if errorlevel 1 goto :notrunning
echo [check] Native engine OK.

echo [run] Executing parity battery (native vs Everything)...
cargo test --release -p instant-file-search-mcp-server -- --ignored parity --nocapture 2>&1 | findstr /C:"DIFF" /C:"mismatches=" /C:"test result"
echo [done]
goto :eof

:notrunning
echo ERROR: instant-file-search-indexer is not running.
echo Start it with: sc.exe start instant-file-search-indexer
echo Or deploy a fresh build with .\scripts\deploy.bat
exit /b 1
