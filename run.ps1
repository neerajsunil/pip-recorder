param(
    [switch]$Fast,
    [switch]$Release,
    [switch]$SoftwareUI
)
$ErrorActionPreference = 'Stop'
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
$cargoPath = if ($cargoCommand) { $cargoCommand.Source } else { Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
if (-not (Test-Path -LiteralPath $cargoPath)) { throw 'Install Rust with the Windows MSVC toolchain first: https://rust-lang.org/tools/install/' }
Push-Location $PSScriptRoot
try {
    $cargoArgs = @('run', '-p', 'fastrecorder', '--locked')
    if ($Release) { $cargoArgs += '--release' }
    elseif ($Fast) { $cargoArgs += @('--profile', 'fast') }
    if ($SoftwareUI) { $cargoArgs += @('--', '--software-ui') }
    & $cargoPath @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "FastRecorder exited with code $LASTEXITCODE" }
} finally { Pop-Location }
