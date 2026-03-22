# Voxlink Portable Build Script
# Creates a .zip that users can extract and run — no installation needed.
#
# Prerequisites: Rust, Build Tools, CMake (same as build-installer.ps1)
# Does NOT require Inno Setup.

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot\..

Write-Host "Building Voxlink portable package..." -ForegroundColor Cyan

# Build
cargo build --release
if ($LASTEXITCODE -ne 0) { Write-Host "Build failed!" -ForegroundColor Red; exit 1 }

# Create portable folder
$version = (Select-String -Path "crates\app_desktop\Cargo.toml" -Pattern '^version = "(.*)"' | ForEach-Object { $_.Matches[0].Groups[1].Value })
$outDir = "target\Voxlink-$version"
if (Test-Path $outDir) { Remove-Item $outDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

Copy-Item "target\release\app_desktop.exe" "$outDir\Voxlink.exe"
Copy-Item "target\release\signaling_server.exe" "$outDir\Voxlink-Server.exe"

# Create a readme
@"
Voxlink $version
===================

Voxlink.exe        - Run this to start the app
Voxlink-Server.exe - Run this to host a server for your group

Quick Start:
1. One person runs Voxlink-Server.exe
2. Everyone runs Voxlink.exe
3. Enter the server host's IP as: ws://<ip>:9090
4. Click Connect, then Create or Join a room

The server also works on the same PC as the app.
For local testing, use: ws://127.0.0.1:9090
"@ | Out-File -Encoding UTF8 "$outDir\README.txt"

# Zip it
$zipPath = "target\Voxlink-$version-portable.zip"
if (Test-Path $zipPath) { Remove-Item $zipPath }
Compress-Archive -Path $outDir -DestinationPath $zipPath

$zip = Get-Item $zipPath
Write-Host ""
Write-Host "Portable package created: $($zip.FullName)" -ForegroundColor Green
Write-Host "Size: $([math]::Round($zip.Length / 1MB, 1)) MB" -ForegroundColor White
Write-Host ""
Write-Host "Send this zip to anyone. They extract and run Voxlink.exe." -ForegroundColor Cyan
