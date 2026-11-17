[CmdletBinding()]
param([switch]$SkipNpmCi)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$previousLocation = Get-Location

try {
    Set-Location $projectRoot
    foreach ($tool in @('node', 'npm.cmd', 'cargo', 'rustc', 'rustup', 'git')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            throw "Missing $tool. See windows/README.md for the Windows build prerequisites."
        }
    }
    $hostInfo = & rustc -vV
    if ($LASTEXITCODE -ne 0 -or -not ($hostInfo -match 'host: x86_64-pc-windows-msvc')) {
        throw 'The Windows preview requires the x64 MSVC Rust toolchain on Windows.'
    }
    $installedTargets = & rustup target list --installed
    if ($LASTEXITCODE -ne 0 -or -not ($installedTargets -contains 'x86_64-pc-windows-msvc')) {
        throw 'Install the target first: rustup target add x86_64-pc-windows-msvc'
    }
    if (-not $SkipNpmCi) {
        & npm.cmd ci
        if ($LASTEXITCODE -ne 0) { throw 'npm ci failed.' }
    }
    & (Join-Path $PSScriptRoot 'Prepare-Runtime.ps1')
    # Tauri merges tauri.windows.conf.json automatically for the Windows target.
    # --locked prevents silently replacing the reviewed Rust dependency set.
    & npm.cmd run tauri -- build --target x86_64-pc-windows-msvc --bundles nsis -- --locked
    if ($LASTEXITCODE -ne 0) { throw 'The Windows preview build failed.' }
    $installerDirectory = Join-Path $projectRoot 'src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis'
    $installers = @(Get-ChildItem -LiteralPath $installerDirectory -Filter 'Sonante Windows Preview 44*_x64-setup.exe' -File)
    if ($installers.Count -ne 1) {
        throw "Expected exactly one x64 NSIS installer in $installerDirectory; found $($installers.Count)."
    }
    & (Join-Path $PSScriptRoot 'Write-Preview-Receipt.ps1') -InstallerPath $installers[0].FullName
    Write-Host "Installer and Preview 44 receipt: $PSScriptRoot/artifacts/"
} finally {
    Set-Location $previousLocation
}
