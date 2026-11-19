@echo off
rem Bygger SMTP-Graph og samler det, der skal kopieres ud til kunden, i mappen udgivelse.
rem Er Inno Setup 6 installeret, laves også installeren udgivelse\SMTP-Graph-Setup-<version>.exe.
cd /d "%~dp0"
cargo build --release || exit /b 1
if not exist udgivelse mkdir udgivelse
copy /y target\release\smtp-graph.exe udgivelse\ >nul
copy /y target\release\smtp-graph-gui.exe udgivelse\ >nul
copy /y Opsaet-SmtpGraph.ps1 udgivelse\ >nul
copy /y README.da.md udgivelse\README.md >nul

rem Version fra Cargo.toml (første linje, der starter med "version").
for /f "tokens=2 delims==" %%v in ('findstr /b /c:"version" Cargo.toml') do if not defined VER set "VER=%%v"
set "VER=%VER: =%"
set "VER=%VER:"=%"

set "ISCC=%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
if not exist "%ISCC%" set "ISCC=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if not exist "%ISCC%" set "ISCC=%ProgramFiles%\Inno Setup 6\ISCC.exe"
if exist "%ISCC%" (
  "%ISCC%" /Q /DVersion=%VER% installer\SMTP-Graph.iss || exit /b 1
  echo Installer: %~dp0udgivelse\SMTP-Graph-Setup-%VER%.exe
) else (
  echo Inno Setup 6 findes ikke - installeren er ikke lavet ^(winget install JRSoftware.InnoSetup^).
)
echo Faerdig: %~dp0udgivelse
