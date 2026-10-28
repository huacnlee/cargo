# Install huacnlee/cargo in place of the rustup `cargo` proxy for the current Windows user.
#   irm https://huacnlee.github.io/cargo/install.ps1 | iex
[CmdletBinding()]
param(
    [string]$Version = 'latest',
    [string]$InstallDir = $(if ($env:CARGO_HOME) { Join-Path $env:CARGO_HOME 'bin' } else { Join-Path $HOME '.cargo\bin' }),
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
if ($env:OS -ne 'Windows_NT') { throw 'This installer requires Windows. Use install.sh on macOS or Linux.' }
$target = Join-Path $InstallDir 'cargo.exe'
$backup = Join-Path $InstallDir 'cargo.rustup.exe'
if ($Uninstall) {
    if (-not (Test-Path -LiteralPath $backup)) { throw "Nothing to restore: $backup does not exist." }
    Move-Item -LiteralPath $backup -Destination $target -Force
    Write-Host "Restored $target"
    return
}
$architecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
$triple = switch ($architecture) {
    'AMD64' { 'x86_64-pc-windows-msvc' }
    default { throw "Unsupported CPU architecture: $architecture (Windows releases support x86_64 only)." }
}
if ($Version -eq 'latest') {
    $release = Invoke-RestMethod -Uri 'https://api.github.com/repos/huacnlee/cargo/releases/latest'
    $Version = $release.tag_name
}
$Version = $Version -replace '^v', ''
if ($Version -notmatch '^\d+\.\d+\.\d+([.-][A-Za-z0-9.-]+)?$') { throw 'Version must be a release version such as 0.102.0-1 or v0.102.0-1.' }
$asset = "cargo-$Version-$triple.zip"
$base = "https://github.com/huacnlee/cargo/releases/download/v$Version"
$work = Join-Path ([IO.Path]::GetTempPath()) ('cargo-install-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    Write-Host "Downloading cargo $Version for $triple…"
    $archive = Join-Path $work $asset
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $archive
    $checksums = Join-Path $work 'SHA256SUMS'
    Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $checksums
    $expected = @(Get-Content $checksums | ForEach-Object {
        if ($_ -match '^([a-fA-F0-9]{64})\s+\*?(.+)$' -and $Matches[2] -ceq $asset) { $Matches[1] }
    })
    if ($expected.Count -ne 1) { throw "Missing or ambiguous checksum for $asset" }
    if ((Get-FileHash $archive -Algorithm SHA256).Hash -ine $expected[0]) { throw 'Checksum verification failed; nothing was installed.' }
    $unpacked = Join-Path $work 'unpacked'
    Expand-Archive -LiteralPath $archive -DestinationPath $unpacked
    $binary = Join-Path $unpacked 'cargo.exe'
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw 'Release archive is missing cargo.exe.' }
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    if ((Test-Path -LiteralPath $target) -and -not (Test-Path -LiteralPath $backup)) {
        Copy-Item -LiteralPath $target -Destination $backup
    }
    # Windows refuses replacement while cargo is running; report the failure.
    Copy-Item -LiteralPath $binary -Destination $target -Force
    Write-Host "Installed $target"
    if (Test-Path -LiteralPath $backup) { Write-Host "Previous cargo kept as $backup (restore with -Uninstall)." }
    # Nightly builds use Cranelift for local crates; it ships as a component.
    if ((Get-Command rustup -ErrorAction SilentlyContinue) -and (rustup toolchain list | Select-String '^nightly')) {
        Write-Host 'Adding the Cranelift backend to the nightly toolchain…'
        rustup component add rustc-codegen-cranelift-preview --toolchain nightly *> $null
        if ($LASTEXITCODE -ne 0) { Write-Host 'Cranelift is not available for this nightly; nightly builds use LLVM.' }
    }
    Write-Host 'Note: `rustup self update` reinstalls its proxies; rerun this script afterwards.'
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
