[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$manifest = Get-Content (Join-Path $PSScriptRoot 'mpd-runtime.json') -Raw | ConvertFrom-Json

# Keep source filenames and package destinations coupled to the reviewed pins.
# Refuse a stale directory/glob mapping before downloading or executing anything.
if ([string]$manifest.version -notmatch '\A[0-9]+\.[0-9]+\.[0-9]+\z') {
    throw 'Invalid MPD runtime version in manifest.'
}
$files = @(
    @{ Name = 'mpd.exe'; Url = $manifest.url; Hash = $manifest.sha256 },
    @{ Name = "mpd-$($manifest.version).tar.xz"; Url = $manifest.sourceUrl; Hash = $manifest.sourceSha256 },
    @{ Name = "mpd-$($manifest.version).exe.sig"; Url = $manifest.binarySignatureUrl; Hash = $manifest.binarySignatureSha256 },
    @{ Name = "mpd-$($manifest.version).tar.xz.sig"; Url = $manifest.sourceSignatureUrl; Hash = $manifest.sourceSignatureSha256 }
)
$projectRoot = Split-Path -Parent $PSScriptRoot
$config = Get-Content (Join-Path $projectRoot 'src-tauri/tauri.windows.conf.json') -Raw | ConvertFrom-Json
$resources = $config.bundle.resources
$expectedSources = @()
foreach ($file in $files) {
    $uri = $null
    if ([string]$file.Hash -notmatch '\A[a-fA-F0-9]{64}\z' -or
        -not [Uri]::TryCreate([string]$file.Url, [UriKind]::Absolute, [ref]$uri) -or $uri.Scheme -ne 'https') {
        throw "Invalid URL or SHA256 pin for $($file.Name)."
    }
    $source = '../windows/runtime/' + $file.Name
    $target = 'runtimes/mpd/' + $file.Name
    $entry = $resources.PSObject.Properties[$source]
    if ($null -eq $entry -or [string]$entry.Value -ne $target) {
        throw "Windows resource mapping differs from the runtime manifest: $($file.Name)"
    }
    $expectedSources += $source
}
# Audio candidates use their own runtime manifests and hash locks. Keep their
# resource mappings explicit while allowing them through the MPD source guard.
$candidateResources = @{
    '../windows/runtime/audio-mpd-candidate-5.exe' = 'runtimes/audio-candidate/mpd.exe'
    '../windows/runtime/audio-source-candidate-5.tar.xz' = 'runtimes/audio-candidate/source-materials.tar.xz'
    '../windows/runtime/camilla-candidate-3.exe' = 'runtimes/audio-candidate/camilladsp.exe'
    '../windows/runtime/camilla-source-candidate-3.tar.xz' = 'runtimes/audio-candidate/camilla-source-materials.tar.xz'
}
foreach ($mapping in $candidateResources.GetEnumerator()) {
    $entry = $resources.PSObject.Properties[$mapping.Key]
    if ($null -eq $entry -or [string]$entry.Value -ne $mapping.Value) {
        throw "Windows candidate resource mapping differs from its pinned destination: $($mapping.Key)"
    }
    $expectedSources += $mapping.Key
}
foreach ($entry in $resources.PSObject.Properties) {
    $source = $entry.Name.Replace('\', '/')
    if (($source -eq '../windows/runtime' -or $source.StartsWith('../windows/runtime/', [StringComparison]::OrdinalIgnoreCase)) -and
        $expectedSources -notcontains $entry.Name) {
        throw 'Windows runtime resources must use explicit pinned file mappings.'
    }
}
$runtime = Join-Path $PSScriptRoot 'runtime'
New-Item -ItemType Directory -Force -Path $runtime | Out-Null

function Get-PinnedFile([string]$Url, [string]$Name, [string]$Expected) {
    $destination = Join-Path $runtime $Name
    if ((Test-Path $destination) -and ((Get-FileHash $destination -Algorithm SHA256).Hash.ToLowerInvariant() -eq $Expected)) { return }
    $temporary = Join-Path $runtime ([Guid]::NewGuid().ToString() + '.download')
    try {
        Invoke-WebRequest -Uri $Url -OutFile $temporary -UseBasicParsing
        if ((Get-FileHash $temporary -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Expected) {
            throw "Runtime hash mismatch: $Name"
        }
        Move-Item -Force $temporary $destination
    } finally {
        if (Test-Path $temporary) { Remove-Item $temporary }
    }
}
foreach ($file in $files) {
    Get-PinnedFile $file.Url $file.Name $file.Hash
}
$versionOutput = @(& (Join-Path $runtime 'mpd.exe') --version 2>&1)
if ($LASTEXITCODE -ne 0 -or $versionOutput.Count -eq 0 -or
    ([string]$versionOutput[0]) -notmatch ('^Music Player Daemon ' + [Regex]::Escape($manifest.version) + '(?:\s|$)')) {
    throw 'MPD binary identity differs from the runtime manifest.'
}
Write-Host "Verified SHA256 pins for MPD $($manifest.version) binary, source and detached signatures."
