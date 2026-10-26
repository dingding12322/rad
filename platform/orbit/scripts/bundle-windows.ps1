# Package the Windows build twice: a zip (the self-contained orbit-pi.exe with
# assets and bundled pi extensions compiled in, the app icon, and the license)
# and the bare orbit-pi.exe on its own, so the release carries a direct
# single-file download as well as the archive.
#
# Usage (from the repo root, in PowerShell):
#   pwsh scripts/bundle-windows.ps1 [-Arch x86_64|aarch64]
#
# Env:
#   VERSION   bundle version (default: crates/orbit-pi/Cargo.toml)
#   ARCH      x86_64 (default) or aarch64
#   ISCC      path to Inno Setup's ISCC.exe (default: auto-detect)

param(
    [ValidateSet("x86_64", "aarch64")]
    [string]$Arch = $(if ($env:ARCH) { $env:ARCH } else { "x86_64" })
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = $env:VERSION
if (-not $version) {
    $match = Select-String -Path "crates/orbit-pi/Cargo.toml" -Pattern '^version = "(.+)"' |
        Select-Object -First 1
    $version = $match.Matches[0].Groups[1].Value
}

$triple = "$Arch-pc-windows-msvc"
# Inno's own arch identifier differs from Rust's target arch name.
$innoArch = if ($Arch -eq "aarch64") { "arm64" } else { "x64" }

cargo build --locked --release -p orbit-pi --target $triple

$release = Join-Path $root ("target\{0}\release" -f $triple)
$exe = Join-Path $release "orbit-pi.exe"
if (-not (Test-Path $exe)) {
    throw "expected $exe after the release build"
}

# Locate Inno Setup's command-line compiler: an explicit $env:ISCC, then PATH,
# then the default install locations for Inno Setup 6 and 5.
function Find-Iscc {
    if ($env:ISCC) { return $env:ISCC }
    $onPath = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $candidates = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 5\ISCC.exe"
    )
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) { return $candidate }
    }
    throw "ISCC.exe not found. Install Inno Setup 6 (https://jrsoftware.org/isdl.php) or set `$env:ISCC."
}

$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force $dist | Out-Null

$iscc = Find-Iscc
Write-Host "Building installer for $version ($Arch)"
& $iscc `
    "/DAppVersion=$version" `
    "/DSourceDir=$release" `
    "/DOutputDir=$dist" `
    "/DTargetArch=$Arch" `
    "/DInnoArch=$innoArch" `
    "scripts/installer/orbit-pi.iss"
if ($LASTEXITCODE -ne 0) {
    throw "ISCC failed with exit code $LASTEXITCODE"
}

# Portable zip: the self-contained exe (assets and bundled pi extensions are
# compiled in), the app icon, and the license.
$package = "orbit-pi-$version-$triple"
$stage = "target/package/$package"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe "$stage/orbit-pi.exe"
Copy-Item "assets/icons/icon.ico" "$stage/icon.ico"
Copy-Item "LICENSE" "$stage/LICENSE"
Compress-Archive -Path "$stage/*" -DestinationPath "dist/$package.zip" -Force

# The bare executable, byte-for-byte what went into the zip above, as its own
# release asset. Copied from the staged tree so the two can never diverge.
Copy-Item "$stage/orbit-pi.exe" "dist/$package.exe" -Force

Write-Host "Created dist/$package.zip"
Write-Host "Created dist/$package.exe"
