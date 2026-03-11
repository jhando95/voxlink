@echo off
:: ============================================================
:: Voxlink — One-Click Windows Build
:: ============================================================
:: Right-click this file and "Run as administrator"
:: It installs everything needed and builds the installer.
:: ============================================================

echo.
echo ========================================
echo   Voxlink Windows Builder
echo ========================================
echo.
echo This will install build tools (if needed) and create
echo the Voxlink installer. This may take 10-20 minutes
echo on first run.
echo.
pause

:: Check for admin
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo ERROR: Please right-click and "Run as administrator"
    pause
    exit /b 1
)

:: Install Rust if missing
where rustc >nul 2>&1
if %errorlevel% neq 0 (
    echo [1/5] Installing Rust...
    winget install Rustlang.Rustup --accept-source-agreements --accept-package-agreements -e
    echo.
    echo Rust installed. Please CLOSE this window and run this script again.
    echo (Rust needs a fresh terminal to work)
    pause
    exit /b 0
) else (
    echo [1/5] Rust already installed
)

:: Install VS Build Tools if missing
where cl >nul 2>&1
if %errorlevel% neq 0 (
    echo [2/5] Installing Visual Studio Build Tools...
    winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --wait" --accept-source-agreements --accept-package-agreements -e
    echo   Build Tools installed
) else (
    echo [2/5] Build Tools already installed
)

:: Install CMake if missing
where cmake >nul 2>&1
if %errorlevel% neq 0 (
    echo [3/5] Installing CMake...
    winget install Kitware.CMake --accept-source-agreements --accept-package-agreements -e
    echo   CMake installed
) else (
    echo [3/5] CMake already installed
)

:: Install Inno Setup if missing
set "ISCC="
if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if exist "%ProgramFiles%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles%\Inno Setup 6\ISCC.exe"
if exist "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" set "ISCC=%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"

if "%ISCC%"=="" (
    echo [4/5] Installing Inno Setup...
    winget install JRSoftware.InnoSetup --accept-source-agreements --accept-package-agreements -e
    echo   Inno Setup installed
) else (
    echo [4/5] Inno Setup already installed
)

echo.
echo [5/5] Building Voxlink...
echo.

:: Refresh PATH for newly installed tools
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"
set "PATH=%ProgramFiles%\CMake\bin;%PATH%"

:: Run the build
cd /d "%~dp0"
powershell -ExecutionPolicy Bypass -File "installer\build-installer.ps1"

if %errorlevel% neq 0 (
    echo.
    echo Build failed. If tools were just installed, close this
    echo window and run the script again.
    pause
    exit /b 1
)

echo.
echo ========================================
echo   DONE! Your installer is ready:
echo   target\installer\Voxlink-Setup-0.1.0.exe
echo ========================================
echo.
echo Send that file to anyone. They install and run.
echo.
explorer target\installer
pause
