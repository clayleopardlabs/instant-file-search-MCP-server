@echo off
setlocal enabledelayedexpansion

rem Deploy fresh release builds of the native indexer and MCP server to the
rem installed bundle. Rebuilds from source, force-restarts the indexer service
rem (SYSTEM-owned), and verifies the deployed binaries are byte-identical to
rem the fresh builds (fc /b) so a stale binary can never be served silently.
rem
rem Run elevated. The script self-elevates when it is not. All output is
rem mirrored to scripts\deploy.log so the detached elevated window stays
rem diagnosable from outside.

set "SCRIPT_DIR=%~dp0"
set "REPO=%~dp0.."
set "SRC=%REPO%\target\release"
set "ROOT=%LOCALAPPDATA%\ClayLeopardLabs\EverythingMCP"
set "SVC=instant-file-search-indexer"
set "LOG=%SCRIPT_DIR%deploy.log"
set "MINGW=C:\Users\Omen\AppData\Local\Microsoft\WinGet\Packages\BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe\mingw64\bin"

echo [%date% %time%] deploy.bat started (pid %random%) > "%LOG%"

net session >nul 2>&1
if %errorlevel% neq 0 (
  echo Requesting administrator rights...
  >> "%LOG%" echo [%date% %time%] not elevated, requesting UAC
  powershell -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
  exit /b
)
>> "%LOG%" echo [%date% %time%] elevated, continuing

cd /d "%REPO%"
if errorlevel 1 (
  >> "%LOG%" echo [%date% %time%] FAILED: cannot cd to %REPO%
  echo FAILED: cannot cd to %REPO%
  pause
  exit /b 1
)
>> "%LOG%" echo [%date% %time%] cwd=%CD%

echo [1/5] Building release binaries (indexer + MCP server)...
set "PATH=%MINGW%;%PATH%"
cargo build --release -p instant-file-search-indexer -p instant-file-search-mcp-server >> "%LOG%" 2>&1
if errorlevel 1 (
  >> "%LOG%" echo [%date% %time%] BUILD FAILED
  echo BUILD FAILED
  pause
  exit /b 1
)
>> "%LOG%" echo [%date% %time%] build OK
if not exist "%SRC%\instant-file-search-indexer.exe" (
  >> "%LOG%" echo [%date% %time%] INDEXER BINARY NOT FOUND
  echo INDEXER BINARY NOT FOUND: %SRC%\instant-file-search-indexer.exe
  pause
  exit /b 1
)
if not exist "%SRC%\instant-file-search-mcp-server.exe" (
  >> "%LOG%" echo [%date% %time%] MCP BINARY NOT FOUND
  echo MCP BINARY NOT FOUND: %SRC%\instant-file-search-mcp-server.exe
  pause
  exit /b 1
)

echo [2/5] Stopping %SVC% service...
>> "%LOG%" echo [%date% %time%] stopping service
taskkill /F /IM instant-file-search-indexer.exe >nul 2>&1
sc.exe stop %SVC% >nul 2>&1
timeout /t 2 /nobreak >nul

echo [3/5] Copying binaries...
if not exist "%ROOT%\indexer" mkdir "%ROOT%\indexer"
copy /Y "%SRC%\instant-file-search-indexer.exe" "%ROOT%\indexer\instant-file-search-indexer.exe" >nul
if errorlevel 1 (
  >> "%LOG%" echo [%date% %time%] COPY FAILED: indexer
  echo COPY FAILED: %ROOT%\indexer\instant-file-search-indexer.exe
  pause
  exit /b 1
)
rem Rename a running MCP server image before overwriting (Windows refuses
rem to overwrite an executing exe but allows renaming it). The plugin picks
rem up the fresh binary on its next spawn.
if exist "%ROOT%\instant-file-search-mcp-server.exe" (
  del /f "%ROOT%\instant-file-search-mcp-server.exe.old" >nul 2>&1
  ren "%ROOT%\instant-file-search-mcp-server.exe" "instant-file-search-mcp-server.exe.old" >nul 2>&1
)
copy /Y "%SRC%\instant-file-search-mcp-server.exe" "%ROOT%\instant-file-search-mcp-server.exe" >nul
if errorlevel 1 (
  >> "%LOG%" echo [%date% %time%] COPY FAILED: MCP server
  echo COPY FAILED: %ROOT%\instant-file-search-mcp-server.exe
  pause
  exit /b 1
)
>> "%LOG%" echo [%date% %time%] copies done

echo [4/5] Verifying deployed binaries match fresh builds (fc /b)...
fc /b "%SRC%\instant-file-search-indexer.exe" "%ROOT%\indexer\instant-file-search-indexer.exe" >nul
if errorlevel 1 (
  >> "%LOG%" echo [%date% %time%] STALE BINARY GUARD TRIPPED: indexer
  echo STALE BINARY GUARD TRIPPED: deployed indexer does not match the fresh build
  pause
  exit /b 1
)
fc /b "%SRC%\instant-file-search-mcp-server.exe" "%ROOT%\instant-file-search-mcp-server.exe" >nul
if errorlevel 1 (
  >> "%LOG%" echo [%date% %time%] STALE BINARY GUARD TRIPPED: MCP server
  echo STALE BINARY GUARD TRIPPED: deployed MCP server does not match the fresh build
  pause
  exit /b 1
)
>> "%LOG%" echo [%date% %time%] fc /b verify OK
echo Deployed binaries verified byte-identical to fresh builds.

echo [5/5] Starting %SVC% service...
sc.exe start %SVC% >nul 2>&1
if errorlevel 1 (
  >> "%LOG%" echo [%date% %time%] SERVICE START FAILED
  echo SERVICE START FAILED
  pause
  exit /b 1
)
>> "%LOG%" echo [%date% %time%] service started
echo Service started. The index rebuilds in the background (~15s for 2.4M files).
echo Verify with: .\scripts\doctor.ps1   or   search_status via the MCP tools.
>> "%LOG%" echo [%date% %time%] deploy.bat finished OK
pause
