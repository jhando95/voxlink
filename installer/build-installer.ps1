# Voxlink Windows Installer Build Script
# Run this on a Windows PC to produce Voxlink-Setup-0.1.0.exe
#
# Prerequisites (developer only — end users need nothing):
#   1. Rust:          winget install Rustlang.Rustup
#   2. Build Tools:   winget install Microsoft.VisualStudio.2022.BuildTools --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"
#   3. CMake:         winget install Kitware.CMake
#   4. Inno Setup:    winget install JRSoftware.InnoSetup
#
# After installing prerequisites, close and reopen PowerShell, then run:
#   .\installer\build-installer.ps1

param(
    [switch]$SkipBuild,
    [switch]$ServerOnly
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot\..

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Voxlink Installer Builder" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# --- Check prerequisites ---
function Check-Command($name) {
    if (-not (Get-Command $name -ErrorAction SilentlyContinue)) {
        Write-Host "ERROR: '$name' not found. Please install it first." -ForegroundColor Red
        return $false
    }
    return $true
}

$ok = $true
if (-not (Check-Command "rustc"))  { $ok = $false; Write-Host "  Install: winget install Rustlang.Rustup" -ForegroundColor Yellow }
if (-not (Check-Command "cmake"))  { $ok = $false; Write-Host "  Install: winget install Kitware.CMake" -ForegroundColor Yellow }
if (-not (Check-Command "cargo"))  { $ok = $false; Write-Host "  Install: winget install Rustlang.Rustup" -ForegroundColor Yellow }

# Find Inno Setup compiler
$iscc = $null
$innoPaths = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles}\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
)
foreach ($p in $innoPaths) {
    if (Test-Path $p) { $iscc = $p; break }
}
if (-not $iscc) {
    Write-Host "ERROR: Inno Setup not found." -ForegroundColor Red
    Write-Host "  Install: winget install JRSoftware.InnoSetup" -ForegroundColor Yellow
    $ok = $false
}

if (-not $ok) {
    Write-Host ""
    Write-Host "Missing prerequisites. Install them and restart PowerShell." -ForegroundColor Red
    exit 1
}

Write-Host "Rust:       $(rustc --version)" -ForegroundColor Gray
Write-Host "CMake:      $(cmake --version | Select-Object -First 1)" -ForegroundColor Gray
Write-Host "Inno Setup: $iscc" -ForegroundColor Gray
Write-Host ""

# --- Download VC++ Redistributable if not cached ---
$redistDir = "installer\redist"
$redistFile = "$redistDir\vc_redist.x64.exe"

if (-not (Test-Path $redistFile)) {
    Write-Host "[0] Downloading VC++ Redistributable..." -ForegroundColor Green
    New-Item -ItemType Directory -Force -Path $redistDir | Out-Null

    $vcUrl = "https://aka.ms/vs/17/release/vc_redist.x64.exe"
    try {
        Invoke-WebRequest -Uri $vcUrl -OutFile $redistFile -UseBasicParsing
        $size = [math]::Round((Get-Item $redistFile).Length / 1MB, 1)
        Write-Host "  Downloaded vc_redist.x64.exe ($size MB)" -ForegroundColor Gray
    } catch {
        Write-Host "ERROR: Failed to download VC++ Redistributable." -ForegroundColor Red
        Write-Host "  Download manually from: $vcUrl" -ForegroundColor Yellow
        Write-Host "  Save to: $redistFile" -ForegroundColor Yellow
        exit 1
    }
    Write-Host ""
} else {
    Write-Host "[0] VC++ Redistributable already cached." -ForegroundColor Gray
    Write-Host ""
}

# --- Build release binaries ---
if (-not $SkipBuild) {
    Write-Host "[1/2] Building release binaries..." -ForegroundColor Green

    if ($ServerOnly) {
        cargo build --release --bin signaling_server
    } else {
        cargo build --release
    }

    if ($LASTEXITCODE -ne 0) {
        Write-Host "Build failed!" -ForegroundColor Red
        exit 1
    }

    # Show binary sizes
    $app = Get-Item "target\release\app_desktop.exe" -ErrorAction SilentlyContinue
    $srv = Get-Item "target\release\signaling_server.exe" -ErrorAction SilentlyContinue
    if ($app) { Write-Host "  app_desktop.exe:      $([math]::Round($app.Length / 1MB, 1)) MB" -ForegroundColor Gray }
    if ($srv) { Write-Host "  signaling_server.exe: $([math]::Round($srv.Length / 1MB, 1)) MB" -ForegroundColor Gray }
    Write-Host ""
} else {
    Write-Host "[1/2] Skipping build (--SkipBuild)" -ForegroundColor Yellow
    Write-Host ""
}

# --- Create installer ---
Write-Host "[2/2] Creating installer..." -ForegroundColor Green

# Ensure output directory exists
New-Item -ItemType Directory -Force -Path "target\installer" | Out-Null

& $iscc "installer\voxlink.iss"

if ($LASTEXITCODE -ne 0) {
    Write-Host "Installer creation failed!" -ForegroundColor Red
    exit 1
}

$installer = Get-Item "target\installer\Voxlink-Setup-*.exe"
Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host "  Installer created successfully!" -ForegroundColor Green
Write-Host "  $($installer.FullName)" -ForegroundColor White
Write-Host "  Size: $([math]::Round($installer.Length / 1MB, 1)) MB" -ForegroundColor White
Write-Host "========================================" -ForegroundColor Green
Write-Host ""
Write-Host "Send this file to anyone. They double-click it to install Voxlink." -ForegroundColor Cyan
Write-Host "No prerequisites needed — VC++ Runtime is bundled." -ForegroundColor Cyan
