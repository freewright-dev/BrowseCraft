@echo off
rem ===========================================================================
rem  BrowseCraft - build script for Windows
rem
rem  Usage:   build.bat [release|debug] [test] [clean] [run]
rem             (no arguments)  optimised release build  -> dist\browsecraft.exe
rem             debug           faster build with a console window for eprintln!
rem             test            run the unit tests instead of building
rem             clean           remove target\ and dist\
rem             run             start the browser after a successful build
rem
rem  Needs: Rust (rustup) with the MSVC toolchain, plus the Visual Studio
rem         "Build Tools" with the "Desktop development with C++" workload.
rem         At run time the embedded web view needs the WebView2 runtime
rem         (already part of Windows 11 and current Windows 10).
rem ===========================================================================
setlocal EnableExtensions
title BrowseCraft build
cd /d "%~dp0"

set "MODE=release"
set "ACTION=build"
set "RUN_AFTER="

:parse
if "%~1"=="" goto :parsed
if /i "%~1"=="debug"   set "MODE=debug"
if /i "%~1"=="release" set "MODE=release"
if /i "%~1"=="test"    set "ACTION=test"
if /i "%~1"=="clean"   set "ACTION=clean"
if /i "%~1"=="run"     set "RUN_AFTER=1"
if /i "%~1"=="help"    goto :usage
if /i "%~1"=="/?"      goto :usage
if /i "%~1"=="-h"      goto :usage
shift
goto :parse
:parsed

if /i "%ACTION%"=="clean" goto :clean

rem ---- 1. Rust toolchain -----------------------------------------------------
where cargo >nul 2>nul
if errorlevel 1 (
    echo [error] Rust was not found on PATH.
    echo         Install it from https://rustup.rs and open a new terminal.
    goto :fail
)

for /f "tokens=2" %%v in ('rustc --version') do set "RUSTVER=%%v"
for /f "tokens=1,2 delims=." %%a in ("%RUSTVER%") do (
    set "RMAJ=%%a"
    set "RMIN=%%b"
)
echo Using rustc %RUSTVER%

rem The egui 0.36 dependency declares a minimum of Rust 1.95.
set "RUST_OK=0"
if %RMAJ% GTR 1 set "RUST_OK=1"
if %RMAJ% EQU 1 if %RMIN% GEQ 95 set "RUST_OK=1"
if "%RUST_OK%"=="0" (
    echo [warn] This project needs Rust 1.95 or newer, found %RUSTVER%.
    where rustup >nul 2>nul
    if errorlevel 1 (
        echo [error] rustup was not found, so Rust cannot be updated automatically.
        echo         Install a current Rust from https://rustup.rs
        goto :fail
    )
    echo Updating the stable toolchain with rustup...
    rustup update stable
    if errorlevel 1 goto :fail
)

rem ---- 2. Tests (optional) ---------------------------------------------------
if /i "%ACTION%"=="test" (
    echo Running unit tests...
    cargo test
    if errorlevel 1 goto :fail
    echo.
    echo All tests passed.
    goto :done
)

rem ---- 3. Build --------------------------------------------------------------
set "CARGO_ARGS="
if /i "%MODE%"=="release" set "CARGO_ARGS=--release"

echo Building BrowseCraft ^(%MODE%^)...
if /i "%MODE%"=="release" echo The release profile uses LTO, so the first build takes a few minutes.
cargo build %CARGO_ARGS%
if errorlevel 1 (
    echo.
    echo [error] The build failed. Common causes:
    echo   - "linker link.exe not found": install the Visual Studio Build Tools
    echo     with the "Desktop development with C++" workload.
    echo   - "requires rustc 1.95": run  rustup update stable
    goto :fail
)

rem ---- 4. Collect a ready-to-run folder --------------------------------------
set "EXE=target\%MODE%\browsecraft.exe"
if not exist "%EXE%" (
    echo [error] Expected %EXE% but it was not produced.
    goto :fail
)
if exist dist rmdir /s /q dist
mkdir dist
copy /y "%EXE%" "dist\browsecraft.exe" >nul
if exist filters xcopy /e /i /y /q filters "dist\filters" >nul

echo.
echo Build finished: %CD%\dist\browsecraft.exe
echo The ad-filter lists in dist\filters are loaded next to the executable.

if defined RUN_AFTER (
    echo Starting BrowseCraft...
    start "" "dist\browsecraft.exe"
)
goto :done

:clean
echo Cleaning build output...
where cargo >nul 2>nul && cargo clean
if exist dist rmdir /s /q dist
echo Done.
goto :done

:usage
echo Usage: build.bat [release^|debug] [test] [clean] [run]
echo   release   optimised build, output in dist\ ^(default^)
echo   debug     faster build, keeps a console window for log output
echo   test      run the unit tests
echo   clean     remove target\ and dist\
echo   run       start the browser after a successful build
goto :done

:fail
echo.
echo Build did not complete.
if not defined NOPAUSE pause
endlocal
exit /b 1

:done
if not defined NOPAUSE pause
endlocal
exit /b 0
