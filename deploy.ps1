<#
.SYNOPSIS
    Builds server-pet for Linux, zips it, and copies the zip to the server's
    home directory.

.DESCRIPTION
    The release build runs inside WSL, because the server runs Linux and a
    Windows build would not start there. The zip contains:

        server-pet/server-pet      the binary, marked executable
        server-pet/assets/         species manifest and sprites
        server-pet/.env.example    configuration template
        server-pet/README.md

    It does not contain .env or the database: secrets and pet data stay where
    they are. Before uploading, the script checks the server's glibc is new
    enough to run a binary built in WSL, and stops if it is not.

    The zip is written to dist\server-pet.zip and uploaded as
    ~/server-pet.zip, replacing any previous upload. Nothing is unzipped or
    restarted on the server.

.PARAMETER Server
    SSH destination, as user@host.

.PARAMETER KeyPath
    Private key for the server.

.PARAMETER Distro
    WSL distribution to build in. It needs Rust (cargo) and python3.

.PARAMETER NoUpload
    Build and package only. No connection to the server is made.

.PARAMETER Force
    Upload even if the server's glibc looks older than the binary needs.

.EXAMPLE
    .\deploy.ps1

.EXAMPLE
    .\deploy.ps1 -NoUpload
#>
[CmdletBinding()]
param(
    [string]$Server = 'admin@52.14.10.147',
    [string]$KeyPath = (Join-Path $env:USERPROFILE 'Downloads\admin.pem'),
    [string]$Distro = 'Debian',
    [switch]$NoUpload,
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ZipName = 'server-pet.zip'
$Root = $PSScriptRoot
$Dist = Join-Path $Root 'dist'
$Zip = Join-Path $Dist $ZipName

# Build steps run in WSL. The script is written to a temporary file and run
# from there, rather than passed on the command line, because Windows
# PowerShell 5.1 and PowerShell 7 quote native arguments differently. Paths
# arrive through WSLENV, which translates them to Linux paths.
$BuildScript = @'
set -euo pipefail

cd "$SP_ROOT"

# Keep build output on the Linux filesystem: much faster than /mnt/c, and it
# never collides with the Windows target directory.
target_dir="$HOME/.cache/server-pet/target"
CARGO_TARGET_DIR="$target_dir" cargo build --release --locked

binary="$target_dir/release/server-pet"

# The newest glibc symbol version the binary links against. A server with an
# older glibc cannot run it.
glibc=$(grep -ao 'GLIBC_[0-9][0-9.]*' "$binary" | sed 's/^GLIBC_//' | sort -Vu | tail -n1)

python3 - "$binary" "$SP_DIST/server-pet.zip" <<'PY'
import os
import sys
import zipfile

binary, out = sys.argv[1], sys.argv[2]

def add(zf, src, arcname, mode=0o644):
    # Store Unix permissions so the binary is still executable once unzipped.
    info = zipfile.ZipInfo.from_file(src, arcname)
    info.external_attr = (0o100000 | mode) << 16
    info.compress_type = zipfile.ZIP_DEFLATED
    with open(src, "rb") as f:
        zf.writestr(info, f.read())

partial = out + ".partial"
with zipfile.ZipFile(partial, "w") as zf:
    add(zf, binary, "server-pet/server-pet", 0o755)
    for base, dirs, files in os.walk("assets"):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(base, name)
            add(zf, path, "server-pet/" + path.replace(os.sep, "/"))
    for extra in (".env.example", "README.md"):
        add(zf, extra, "server-pet/" + extra)

# Only replace the previous zip once the new one is complete.
os.replace(partial, out)
PY

echo "GLIBC_REQUIRED=$glibc"
'@

function Assert-LastExitCode([string]$What) {
    if ($LASTEXITCODE -ne 0) {
        throw "$What failed with exit code $LASTEXITCODE."
    }
}

# -- preflight -----------------------------------------------------------------

if (-not (Get-Command wsl.exe -ErrorAction SilentlyContinue)) {
    throw 'WSL is not installed; it is needed to build the Linux binary.'
}
if (-not $NoUpload) {
    foreach ($tool in 'ssh.exe', 'scp.exe') {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "$tool was not found. Install the Windows OpenSSH client."
        }
    }
    if (-not (Test-Path -LiteralPath $KeyPath -PathType Leaf)) {
        throw "SSH key not found at $KeyPath."
    }
}

New-Item -ItemType Directory -Force -Path $Dist | Out-Null

# -- build and package in WSL --------------------------------------------------

$scriptFile = Join-Path ([IO.Path]::GetTempPath()) "server-pet-build-$PID.sh"
$previousWslEnv = $env:WSLENV
try {
    # Bash rejects carriage returns, so write Unix line endings, without a BOM.
    [IO.File]::WriteAllText($scriptFile, ($BuildScript -replace "`r`n", "`n"), (New-Object Text.UTF8Encoding $false))

    $env:SP_ROOT = $Root
    $env:SP_DIST = $Dist
    $env:WSLENV = (@('SP_ROOT/p', 'SP_DIST/p', $previousWslEnv) | Where-Object { $_ }) -join ':'

    $linuxScript = (& wsl.exe -d $Distro -e wslpath -a $scriptFile | Out-String).Trim()
    Assert-LastExitCode "Finding the build script inside WSL ($Distro)"

    Write-Host "Building a Linux release binary in WSL ($Distro)..." -ForegroundColor Cyan
    # A login shell, so cargo is on PATH. Cargo's progress goes to stderr and
    # shows live; only the script's final report lands in $buildOutput.
    $buildOutput = & wsl.exe -d $Distro -e bash -l $linuxScript
    Assert-LastExitCode 'The Linux build'
}
finally {
    $env:WSLENV = $previousWslEnv
    Remove-Item Env:SP_ROOT, Env:SP_DIST -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $scriptFile -ErrorAction SilentlyContinue
}

$requiredGlibc = $null
foreach ($line in @($buildOutput)) {
    if ("$line" -match '^GLIBC_REQUIRED=(\d+(\.\d+)+)') {
        $requiredGlibc = [version]$Matches[1]
    }
}

$sizeMb = [math]::Round((Get-Item -LiteralPath $Zip).Length / 1MB, 1)
Write-Host "Packaged $Zip ($sizeMb MB)." -ForegroundColor Green
if ($requiredGlibc) {
    Write-Host "The binary needs glibc $requiredGlibc or newer."
}

if ($NoUpload) {
    Write-Host 'Skipping upload (-NoUpload).'
    return
}

# -- upload --------------------------------------------------------------------

# BatchMode fails fast instead of waiting at a password prompt.
# StrictHostKeyChecking=accept-new trusts the server's host key on first
# contact, then refuses to connect if that key ever changes.
$sshOptions = @('-i', $KeyPath, '-o', 'BatchMode=yes', '-o', 'StrictHostKeyChecking=accept-new')

if ($requiredGlibc) {
    Write-Host "Checking glibc on $Server..." -ForegroundColor Cyan
    $remoteLine = (& ssh.exe @sshOptions $Server 'ldd --version 2>&1 | head -n 1' | Out-String).Trim()
    Assert-LastExitCode "Connecting to $Server"

    if ($remoteLine -match '(\d+\.\d+)\s*$') {
        $serverGlibc = [version]$Matches[1]
        if ($serverGlibc -lt $requiredGlibc) {
            $message = "The server has glibc $serverGlibc, but the binary needs $requiredGlibc. " +
                "It will fail to start with a 'GLIBC_$requiredGlibc not found' error. " +
                'Build in a WSL distro no newer than the server, or rerun with -Force to upload anyway.'
            if (-not $Force) {
                throw $message
            }
            Write-Warning $message
        }
        else {
            Write-Host "Server glibc $serverGlibc is new enough."
        }
    }
    else {
        Write-Warning "Could not read the server's glibc version from: $remoteLine"
    }
}

Write-Host "Uploading to ${Server}:~/$ZipName..." -ForegroundColor Cyan
# A relative remote path lands in the login user's home directory.
& scp.exe @sshOptions $Zip "${Server}:$ZipName"
Assert-LastExitCode 'Uploading the zip'

Write-Host "Uploaded to ~/$ZipName on $Server." -ForegroundColor Green
Write-Host 'To unpack it on the server, keeping an existing .env and data directory:'
Write-Host "  ssh -i `"$KeyPath`" $Server"
Write-Host "  unzip -o ~/$ZipName -d ~"
