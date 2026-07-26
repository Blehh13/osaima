@echo off
REM =====================================================================
REM  OSAIMA / Interstellar OS - Desktop Shell launcher (Windows)
REM  Starts the frontend web server and opens the shell in your browser.
REM
REM  Backend note: the real MCP daemon (ai-core/mcp-daemon) uses Linux
REM  Unix-domain sockets and only runs on Linux/WSL. On Windows the shell
REM  auto-falls back to built-in mock system data, so the full UI, window
REM  manager, apps, RAG and behavior learning all work for the demo.
REM =====================================================================

title OSAIMA - Interstellar OS Shell
cd /d "%~dp0ui\osaima-shell"

set "PORT=8777"
set "URL=http://localhost:%PORT%/index.html"

echo(
echo  ============================================================
echo    OSAIMA  /  Interstellar OS  -  Desktop Shell
echo  ============================================================
echo(
echo    Frontend : ui\osaima-shell  (Lua window manager + apps)
echo    Backend  : mock mode on Windows (real MCP daemon = Linux)
echo    URL      : %URL%
echo(
echo    Demo keys once it opens:
echo      Alt+Return terminal   Alt+a assistant   Alt+r knowledge (RAG)
echo      Alt+b behavior        Alt+z tasks       Alt+Tab cycle layout
echo      Alt+1..5 workspaces    Space  AI launcher
echo  ============================================================
echo(

REM ---- find a Python launcher ----------------------------------------
set "PYCMD="
py -3 --version >nul 2>&1
if %errorlevel%==0 set "PYCMD=py -3"
if not defined PYCMD (
  python --version >nul 2>&1
  if %errorlevel%==0 set "PYCMD=python"
)

if defined PYCMD (
  echo  Starting server with Python on port %PORT% ...
  start "OSAIMA Server" %PYCMD% -m http.server %PORT% --bind 127.0.0.1
  goto :open
)

REM ---- fallback: Node (npx serve) ------------------------------------
where npx >nul 2>&1
if %errorlevel%==0 (
  echo  Python not found. Starting server with Node ^(npx serve^) ...
  start "OSAIMA Server" cmd /c "npx --yes serve -l %PORT% ."
  goto :open
)

echo(
echo  ERROR: Neither Python nor Node.js found on this PC.
echo  Install Python from https://www.python.org/downloads/ ^(tick "Add to PATH"^)
echo  then run this file again.
echo(
pause
goto :eof

:open
REM give the server a moment, then open the browser
timeout /t 2 /nobreak >nul
start "" "%URL%"
echo(
echo  Server is running in a separate "OSAIMA Server" window.
echo  Your browser should now show the shell.
echo(
echo  To STOP the demo: close the "OSAIMA Server" window.
echo(
pause
