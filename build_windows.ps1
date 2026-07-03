# Quick build script for Tabular (native Windows)
# Usage: .\build_windows.ps1 [-Arch x64|arm64|all] [-Deps] [-Clean] [-NoMsi] [-Help]
# Requires: Rust (rustup + cargo) with MSVC toolchain
#           WiX Toolset v4+ for MSI: dotnet tool install --global wix
#
# Output (dist\windows):
#   Tabular-<version>-windows-x86_64.msi   / ...-aarch64.msi   (installer)
#   tabular-<version>-windows-x86_64.zip   / ...-aarch64.zip   (manual install)
#   Tabular-<version>-windows-x86_64.exe   / ...-aarch64.exe   (auto-updater)

[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64', 'all')]
    [string]$Arch = 'x64',
    [switch]$Deps,
    [switch]$Clean,
    [switch]$NoMsi,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'

$AppName    = 'Tabular'
$RepoRoot   = $PSScriptRoot
$DistDir    = Join-Path $RepoRoot 'dist\windows'
$TargetX64  = 'x86_64-pc-windows-msvc'
$TargetArm  = 'aarch64-pc-windows-msvc'

function Write-Info    ($msg) { Write-Host "[INFO] "    -ForegroundColor Blue   -NoNewline; Write-Host $msg }
function Write-Success ($msg) { Write-Host "[SUCCESS] " -ForegroundColor Green  -NoNewline; Write-Host $msg }
function Write-Warn    ($msg) { Write-Host "[WARNING] " -ForegroundColor Yellow -NoNewline; Write-Host $msg }
function Write-Err     ($msg) { Write-Host "[ERROR] "   -ForegroundColor Red    -NoNewline; Write-Host $msg }

function Show-Help {
    Write-Host @"
🛠️  Tabular Windows Build Script
================================

Usage: .\build_windows.ps1 [OPTIONS]

Options:
  -Arch x64|arm64|all   Target architecture (default: x64)
  -Deps                 Install rustup targets first
  -Clean                Run 'cargo clean' and clear dist\windows first
  -NoMsi                Skip .msi installer (zip + exe only)
  -Help                 Show this help message

MSI membutuhkan WiX Toolset v4+ (sekali saja):
  dotnet tool install --global wix

Examples:
  .\build_windows.ps1                    # Build x64: msi + zip + exe
  .\build_windows.ps1 -Arch all -Deps    # Install targets, build x64 + arm64
  .\build_windows.ps1 -Clean -NoMsi      # Clean, build x64 tanpa installer
"@
}

if ($Help) { Show-Help; exit 0 }

# --- Version dari Cargo.toml supaya sinkron -------------------------------
$cargoToml = Join-Path $RepoRoot 'Cargo.toml'
$versionLine = Select-String -Path $cargoToml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $versionLine) {
    Write-Err "Tidak bisa membaca version dari Cargo.toml"
    exit 1
}
$Version = $versionLine.Matches[0].Groups[1].Value

# --- Dependency check ------------------------------------------------------
Write-Info "Checking dependencies..."
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Err "Cargo is not installed. Install Rust first: https://rustup.rs"
    exit 1
}
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    Write-Err "rustup is not installed. Install Rust first: https://rustup.rs"
    exit 1
}
if (-not $NoMsi -and -not (Get-Command wix -ErrorAction SilentlyContinue)) {
    Write-Err "WiX CLI not found (needed for .msi). Install with:"
    Write-Err "  dotnet tool install --global wix"
    Write-Err "Or skip the installer with: .\build_windows.ps1 -NoMsi"
    exit 1
}
Write-Success "All dependencies are available!"

# --- Resolve targets -------------------------------------------------------
$targets = switch ($Arch) {
    'x64'   { @($TargetX64) }
    'arm64' { @($TargetArm) }
    'all'   { @($TargetX64, $TargetArm) }
}

if ($Deps) {
    Write-Info "Installing rustup targets..."
    rustup target add @targets
    if ($LASTEXITCODE -ne 0) { Write-Err "rustup target add failed"; exit 1 }
}

if ($Clean) {
    Write-Info "Cleaning previous builds..."
    cargo clean
    if (Test-Path $DistDir) { Remove-Item -Recurse -Force $DistDir }
}

New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

# --- Build + package -------------------------------------------------------
Write-Info "Starting build for $AppName v$Version ($($targets -join ', '))"

foreach ($target in $targets) {
    $archName = if ($target -eq $TargetX64) { 'x86_64' } else { 'aarch64' }

    Write-Info "🪟 Building $target..."
    cargo build --release --target $target
    if ($LASTEXITCODE -ne 0) {
        Write-Err "Build failed for $target"
        exit 1
    }

    $exeSrc = Join-Path $RepoRoot "target\$target\release\tabular.exe"
    if (-not (Test-Path $exeSrc)) {
        Write-Err "Binary not found: $exeSrc"
        exit 1
    }

    # Plain binary (nama sama dengan output Makefile)
    $exePlain = Join-Path $DistDir "tabular-$archName.exe"
    Copy-Item $exeSrc $exePlain -Force

    # Zip archive (manual installation)
    $zipPath = Join-Path $DistDir "tabular-$Version-windows-$archName.zip"
    if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
    Compress-Archive -Path $exePlain -DestinationPath $zipPath

    # Standalone executable (auto-updater)
    Copy-Item $exePlain (Join-Path $DistDir "$AppName-$Version-windows-$archName.exe") -Force

    # MSI installer (WiX v4+)
    if (-not $NoMsi) {
        Write-Info "📦 Building MSI installer for $archName..."
        $wixArch  = if ($archName -eq 'x86_64') { 'x64' } else { 'arm64' }
        $iconPath = Join-Path $RepoRoot 'assets\tabular.ico'
        $msiPath  = Join-Path $DistDir "$AppName-$Version-windows-$archName.msi"
        wix build (Join-Path $RepoRoot 'wix\main.wxs') `
            -arch $wixArch `
            -d "Version=$Version" `
            -d "ExePath=$exeSrc" `
            -d "IconPath=$iconPath" `
            -o $msiPath
        if ($LASTEXITCODE -ne 0) {
            Write-Err "MSI build failed for $archName"
            exit 1
        }
    }

    Write-Success "$archName build packaged."
}

# --- Show results -----------------------------------------------------------
Write-Host ""
Write-Success "🎉 Build completed successfully!"
Write-Host ""
Write-Info "📦 Generated files:"
Get-ChildItem $DistDir -File | Where-Object { $_.Extension -in '.msi', '.zip', '.exe' } | ForEach-Object {
    $size = '{0:N1} MB' -f ($_.Length / 1MB)
    Write-Host "  📁 $($_.FullName) ($size)"
}
Write-Host ""
Write-Info "✨ Ready for distribution!"
