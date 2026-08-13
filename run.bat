@echo off
setlocal
rem Start the MiniDAW Tauri development application.

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
rem Keep Rust's heavily-mutated build cache outside the Desktop workspace.
rem Antivirus/indexers can briefly lock incremental object files in src-tauri\target.
set "CARGO_TARGET_DIR=C:\tmp\minidaw-msvc-target"
if not exist "%USERPROFILE%\.cargo\bin\cargo.exe" (
  echo [MiniDAW] Cargo was not found at "%USERPROFILE%\.cargo\bin\cargo.exe".
  echo Install Rust with rustup, then run this file again.
  pause
  exit /b 1
)

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if exist "%VSWHERE%" (
  for /f "usebackq tokens=*" %%I in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSINSTALL=%%I"
)

if defined VSINSTALL (
  call "%VSINSTALL%\Common7\Tools\VsDevCmd.bat" -arch=amd64 -host_arch=amd64
  if errorlevel 1 exit /b 1
) else (
  echo [MiniDAW] Visual Studio C++ Build Tools were not found.
  pause
  exit /b 1
)

where cargo.exe >nul 2>&1
if errorlevel 1 (
  echo [MiniDAW] Cargo is not available after environment setup.
  pause
  exit /b 1
)

if /i "%~1"=="--check" (
  cargo --version
  cargo metadata --manifest-path src-tauri\Cargo.toml --no-deps --format-version 1 >nul
  if errorlevel 1 exit /b 1
  echo [MiniDAW] Rust and Visual Studio environments are ready.
  exit /b 0
)

rem npm consumes the first --; the second one forwards Cargo runner options.
call npm.cmd run tauri -- dev -- --profile dev-dsp
set "EXIT_CODE=%errorlevel%"
if not "%EXIT_CODE%"=="0" pause
exit /b %EXIT_CODE%
