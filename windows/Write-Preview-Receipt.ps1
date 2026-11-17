[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InstallerPath,
    [string]$BuildCommand = 'powershell -NoProfile -ExecutionPolicy Bypass -File windows/Build-Preview.ps1',
    [string]$OutputDirectory = ''
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$installerDirectory = (Resolve-Path (Join-Path $projectRoot 'src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis')).Path
$resolvedInstaller = (Resolve-Path -LiteralPath $InstallerPath).Path
if (-not $resolvedInstaller.StartsWith($installerDirectory + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
    [IO.Path]::GetExtension($resolvedInstaller) -ne '.exe' -or
    [IO.Path]::GetFileName($resolvedInstaller) -notmatch '[-_]x64-setup\.exe$') {
    throw 'Installer must be an x64 NSIS setup executable inside the Windows bundle output directory.'
}

$milestoneSource = Get-Content (Join-Path $projectRoot 'src-tauri/src/windows.rs') -Raw
$milestones = [regex]::Matches($milestoneSource, 'milestone:\s*"windows-shared-ui-preview-(\d+)"')
if ($milestones.Count -ne 1 -or $milestones[0].Groups[1].Value -ne '44') {
    throw 'The source must declare exactly one Windows Preview 44 milestone.'
}

$artifactDirectory = if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    Join-Path $PSScriptRoot 'artifacts'
} else {
    [IO.Path]::GetFullPath($OutputDirectory)
}
New-Item -ItemType Directory -Force -Path $artifactDirectory | Out-Null
$artifactName = 'Sonante-Windows-Preview-44-x64-setup.exe'
$artifactPath = Join-Path $artifactDirectory $artifactName
$receiptPath = Join-Path $artifactDirectory 'Sonante-Windows-Preview-44-build-receipt.json'
if ((Test-Path -LiteralPath $artifactPath) -or (Test-Path -LiteralPath $receiptPath)) {
    throw 'Preview 44 installer or receipt already exists; refusing to overwrite it.'
}

$branch = (& git -C $projectRoot branch --show-current).Trim()
if ($LASTEXITCODE -ne 0 -or $branch -ne 'develop/0.6.2-unified') {
    throw "Expected branch develop/0.6.2-unified; found '$branch'."
}
$head = (& git -C $projectRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $head -notmatch '^[0-9a-f]{40}$') { throw 'Could not identify the source commit.' }

$changes = @()
foreach ($line in (& git -C $projectRoot status --porcelain=v1 --untracked-files=all)) {
    if ($line.Length -lt 4) { continue }
    $state = $line.Substring(0, 2).Trim()
    $relativePath = $line.Substring(3)
    if ($relativePath.Contains(' -> ')) { $relativePath = $relativePath.Split(' -> ')[-1] }
    $fullPath = Join-Path $projectRoot $relativePath
    if (Test-Path -LiteralPath $fullPath -PathType Leaf) {
        $contentHash = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
    } elseif ($state.Contains('D')) {
        $contentHash = $null
    } else {
        throw "Could not fingerprint changed source file: $relativePath"
    }
    $changes += [ordered]@{ state = $state; path = $relativePath.Replace('\', '/'); sha256 = $contentHash }
}
$changes = @($changes | Sort-Object path)
$changeBytes = [Text.Encoding]::UTF8.GetBytes((ConvertTo-Json -InputObject $changes -Depth 5 -Compress))
$changeHasher = [Security.Cryptography.SHA256]::Create()
try { $changeSha = ([BitConverter]::ToString($changeHasher.ComputeHash($changeBytes))).Replace('-', '').ToLowerInvariant() }
finally { $changeHasher.Dispose() }

$runtimeInputs = @(
    @{ Path = 'windows/runtime/mpd.exe'; Manifest = 'windows/mpd-runtime.json'; Pin = 'sha256' },
    @{ Path = 'windows/runtime/mpd-0.24.18.tar.xz'; Manifest = 'windows/mpd-runtime.json'; Pin = 'sourceSha256' },
    @{ Path = 'windows/runtime/mpd-0.24.18.exe.sig'; Manifest = 'windows/mpd-runtime.json'; Pin = 'binarySignatureSha256' },
    @{ Path = 'windows/runtime/mpd-0.24.18.tar.xz.sig'; Manifest = 'windows/mpd-runtime.json'; Pin = 'sourceSignatureSha256' },
    @{ Path = 'windows/runtime/audio-mpd-candidate-5.exe'; Manifest = 'windows/audio-runtime.json'; Pin = 'sha256' },
    @{ Path = 'windows/runtime/audio-source-candidate-5.tar.xz'; Manifest = 'windows/audio-runtime.json'; Pin = 'sourceMaterialsSha256' },
    @{ Path = 'windows/runtime/camilla-candidate-3.exe'; Manifest = 'windows/camilla-runtime.json'; Pin = 'sha256' },
    @{ Path = 'windows/runtime/camilla-source-candidate-3.tar.xz'; Manifest = 'windows/camilla-runtime.json'; Pin = 'sourceMaterialsSha256' }
)
$runtimeRecords = @()
foreach ($item in $runtimeInputs) {
    $manifest = Get-Content (Join-Path $projectRoot $item.Manifest) -Raw | ConvertFrom-Json
    $expected = [string]$manifest.($item.Pin)
    $runtimePath = Join-Path $projectRoot $item.Path
    if ($expected -notmatch '^[a-fA-F0-9]{64}$' -or -not (Test-Path -LiteralPath $runtimePath -PathType Leaf)) {
        throw "Missing or invalid pinned Windows runtime input: $($item.Path)"
    }
    $actual = (Get-FileHash -LiteralPath $runtimePath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected.ToLowerInvariant()) { throw "Pinned runtime hash mismatch: $($item.Path)" }
    $runtimeRecords += [ordered]@{ path = $item.Path; manifest = $item.Manifest; sha256 = $actual }
}

$packageVersion = [string]((Get-Content (Join-Path $projectRoot 'package.json') -Raw | ConvertFrom-Json).version)
$cargoVersionLine = Select-String -Path (Join-Path $projectRoot 'src-tauri/Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
$cargoVersion = $cargoVersionLine.Matches[0].Groups[1].Value
if ($packageVersion -ne $cargoVersion) { throw 'Cargo and frontend package versions do not agree.' }
$windowsConfig = Get-Content (Join-Path $projectRoot 'src-tauri/tauri.windows.conf.json') -Raw | ConvertFrom-Json
if ($windowsConfig.bundle.windows.nsis.installMode -ne 'currentUser' -or
    $windowsConfig.app.windows.Count -ne 1 -or
    $windowsConfig.app.windows[0].title -ne 'Sonante — Windows Preview 44') {
    throw 'NSIS user-scope or Preview 44 window identity differs from the reviewed configuration.'
}

$temporaryInstaller = Join-Path $artifactDirectory ('.preview44-' + [Guid]::NewGuid().ToString('N') + '.tmp')
$temporaryReceipt = Join-Path $artifactDirectory ('.preview44-' + [Guid]::NewGuid().ToString('N') + '.tmp')
try {
    Copy-Item -LiteralPath $resolvedInstaller -Destination $temporaryInstaller
    $installerHash = (Get-FileHash -LiteralPath $temporaryInstaller -Algorithm SHA256).Hash.ToLowerInvariant()
    $installerBytes = (Get-Item -LiteralPath $temporaryInstaller -Force).Length
    $nodeVersion = & node --version 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Could not record Node.js version.' }
    if ([Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([Runtime.InteropServices.OSPlatform]::Windows)) {
        $npmVersion = & npm.cmd --version 2>&1
    } else {
        $npmVersion = & npm --version 2>&1
    }
    if ($LASTEXITCODE -ne 0) { throw 'Could not record npm version.' }
    $rustcVersion = & rustc --version 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Could not record Rust compiler version.' }
    $cargoVersionOutput = & cargo --version 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Could not record Cargo version.' }
    $toolVersions = [ordered]@{
        node = ([string]::Join(' ', @($nodeVersion))).Trim()
        npm = ([string]::Join(' ', @($npmVersion))).Trim()
        rustc = ([string]::Join(' ', @($rustcVersion))).Trim()
        cargo = ([string]::Join(' ', @($cargoVersionOutput))).Trim()
    }
    $receipt = [ordered]@{
        format = 'sonante-windows-build-receipt-v1'
        created_utc = [DateTimeOffset]::UtcNow.ToString('o')
        identity = [ordered]@{ product = [string]$windowsConfig.productName; window_title = [string]$windowsConfig.app.windows[0].title; preview = 44; package_version = $packageVersion; public_linux_release = '0.6.3'; version_note = 'Preview ordinal and Linux release version are separate identifiers.' }
        source = [ordered]@{ branch = $branch; head = $head; dirty = ($changes.Count -gt 0); changed_files = $changes; changed_files_fingerprint = $changeSha }
        build = [ordered]@{ command = $BuildCommand; host = [ordered]@{ os = [Environment]::OSVersion.VersionString; architecture = $env:PROCESSOR_ARCHITECTURE; powershell = $PSVersionTable.PSVersion.ToString() }; tools = $toolVersions; native_windows_tests_executed = $false }
        pinned_runtime_inputs = $runtimeRecords
        installer = [ordered]@{ name = $artifactName; bytes = $installerBytes; sha256 = $installerHash; signed = $false }
    }
    $json = ConvertTo-Json -InputObject $receipt -Depth 10
    [IO.File]::WriteAllText($temporaryReceipt, $json + [Environment]::NewLine, (New-Object Text.UTF8Encoding($false)))
    Move-Item -LiteralPath $temporaryInstaller -Destination $artifactPath
    Move-Item -LiteralPath $temporaryReceipt -Destination $receiptPath
} finally {
    Remove-Item -LiteralPath $temporaryInstaller, $temporaryReceipt -Force -ErrorAction SilentlyContinue
}
Write-Host "Installer: $artifactPath"
Write-Host "Receipt: $receiptPath"
