param(
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?$')][string]$Version,
    [string]$Binary = 'target/release/fastrecorder.exe'
)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargoPath = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' }
Push-Location $projectRoot
try {
    $binaryPath = [IO.Path]::GetFullPath($Binary)
    if (!(Test-Path -LiteralPath $binaryPath -PathType Leaf)) { throw "Build first; executable missing: $binaryPath" }
    # This packaging path deliberately supports only the validated release target.
    $bytes = [IO.File]::ReadAllBytes($binaryPath)
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    if ([BitConverter]::ToUInt16($bytes, $peOffset + 4) -ne 0x8664) { throw 'Expected a Windows x64 executable.' }
    $distribution = Join-Path $projectRoot 'dist'
    $name = "FastRecorder-$Version-windows-x64"
    $staging = Join-Path $distribution $name
    if (Test-Path -LiteralPath $staging) { throw "Package directory exists; choose a new version or move it first: $staging" }
    New-Item -ItemType Directory -Path $staging -Force | Out-Null
    Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $staging 'fastrecorder.exe')
    foreach ($file in @('LICENSE','README.md','CHANGELOG.md','THIRD_PARTY_NOTICES.md','SUPPORT.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot $file) -Destination $staging
    }
    # Include linked documentation and image assets so the shipped README works offline.
    Copy-Item -LiteralPath (Join-Path $projectRoot 'docs') -Destination (Join-Path $staging 'docs') -Recurse
    $vendored = Join-Path $staging 'licenses/vendored'
    New-Item -ItemType Directory -Path $vendored -Force | Out-Null
    foreach ($item in @(
        @('crates/app/ui/icons/LICENSE','lucide-LICENSE'),
        @('crates/platform-windows/src/encode/nvenc/api/LICENSE','nvenc-adaptation-LICENSE'),
        @('crates/platform-windows/src/encode/nvenc/api/NVIDIA-LICENSE','NVIDIA-LICENSE'),
        @('crates/platform-windows/src/encode/nvenc/api/README.md','nvenc-provenance.md'),
        @('crates/platform-windows/vendor/INTEL-LICENSE','INTEL-LICENSE'),
        @('crates/platform-windows/vendor/README.md','intel-provenance.md')
    )) { Copy-Item -LiteralPath (Join-Path $projectRoot $item[0]) -Destination (Join-Path $vendored $item[1]) }
    $metadata = & $cargoPath metadata --format-version 1 --locked --filter-platform x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Could not read locked dependency metadata.' }
    $metadata = $metadata | ConvertFrom-Json
    $lines = @('# Locked dependency licenses', '', 'SPDX expressions are package metadata. Original license/notice files accompany this package when available.', '', '| Package | Version | License |', '|---|---|---|')
    foreach ($package in ($metadata.packages | Sort-Object name,version)) {
        $license = if ($package.license) { $package.license } else { 'See package license file' }
        $lines += "| $($package.name) | $($package.version) | $license |"
        if (!$package.source) { continue }
        $packageRoot = Split-Path -Parent $package.manifest_path
        $notices = Get-ChildItem -LiteralPath $packageRoot -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)([._-].*)?$' }
        if ($notices) {
            $folder = Join-Path $staging "licenses/dependencies/$($package.name)-$($package.version)"
            New-Item -ItemType Directory -Path $folder -Force | Out-Null
            foreach ($notice in $notices) { Copy-Item -LiteralPath $notice.FullName -Destination $folder }
        }
    }
    $lines | Set-Content -LiteralPath (Join-Path $staging 'DEPENDENCY_LICENSES.md') -Encoding utf8
    $toolchain = & $cargoPath --version
    @("FastRecorder $Version", "Target: x86_64-pc-windows-msvc", "Toolchain: $toolchain", "Source: https://github.com/neerajsunil/pip-recorder/tree/v$Version", 'Unsigned preview binary. See README and release readiness before relying on a recording.') |
        Set-Content -LiteralPath (Join-Path $staging 'BUILD_INFO.txt') -Encoding utf8
    $zip = Join-Path $distribution "$name.zip"
    Compress-Archive -LiteralPath $staging -DestinationPath $zip -CompressionLevel Optimal
    $checksums = Join-Path $distribution "$name.sha256"
    @("$((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLowerInvariant())  $name.zip", "$((Get-FileHash -LiteralPath (Join-Path $staging 'fastrecorder.exe') -Algorithm SHA256).Hash.ToLowerInvariant())  fastrecorder.exe") |
        Set-Content -LiteralPath $checksums -Encoding ascii
    Write-Output "Package: $zip"
    Write-Output "Checksums: $checksums"
} finally { Pop-Location }
