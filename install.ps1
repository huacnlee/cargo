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
# Reads the file instead of running it: older builds recurse when run as rustup.
function Test-ForkCargo([string]$Path) {
    [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($Path)).Contains('huacnlee/cargo')
}
# Earlier versions of this script overwrote rustup.exe with this cargo, which
# breaks rustup and every proxy. Puts the real one back.
function Restore-Rustup {
    $rustup = Join-Path $InstallDir 'rustup.exe'
    if (-not (Test-Path -LiteralPath $rustup) -or -not (Test-ForkCargo $rustup)) { return }
    $real = @($backup, $target) | Where-Object { (Test-Path -LiteralPath $_) -and -not (Test-ForkCargo $_) } | Select-Object -First 1
    if ($real) {
        Copy-Item -LiteralPath $real -Destination "$rustup.staged"
        Move-Item -LiteralPath "$rustup.staged" -Destination $rustup -Force
        Write-Host "Restored $rustup from $real"
    } else {
        Write-Warning "$rustup is a copy of this cargo and no original is left; reinstall rustup from https://rustup.rs."
    }
}
if ($Uninstall) {
    if (-not (Test-Path -LiteralPath $backup)) { throw "Nothing to restore: $backup does not exist." }
    if (Test-ForkCargo $backup) { throw "$backup is huacnlee/cargo, not the original; reinstall rustup from https://rustup.rs." }
    Restore-Rustup
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
    Write-Host "Downloading cargo $Version for $triple..."
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
    Restore-Rustup
    # The backup must be what this cargo replaced, never a build of this cargo.
    if ((Test-Path -LiteralPath $backup) -and (Test-ForkCargo $backup)) {
        Remove-Item -LiteralPath $backup -Force
    }
    if ((Test-Path -LiteralPath $target) -and -not (Test-Path -LiteralPath $backup) -and -not (Test-ForkCargo $target)) {
        Copy-Item -LiteralPath $target -Destination $backup
    }
    # cargo.exe is usually a link to rustup.exe, and copying onto it would write
    # through the link and overwrite rustup.exe, so stage the file and move it.
    # Windows refuses replacement while cargo is running; report the failure.
    $staged = Join-Path $InstallDir ".cargo-staged-$PID.exe"
    Copy-Item -LiteralPath $binary -Destination $staged
    try {
        Move-Item -LiteralPath $staged -Destination $target -Force
    } finally {
        Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
    }
    Write-Host "Installed $target"
    if (Test-Path -LiteralPath $backup) { Write-Host "Previous cargo kept as $backup (restore with -Uninstall)." }
    # Nightly builds use Cranelift for local crates; it ships as a component.
    # rustup logs to stderr, which Windows PowerShell turns into errors. Under
    # `Stop` the first log line would abort the script.
    $ErrorActionPreference = 'Continue'
    $rustupCommand = Get-Command rustup -ErrorAction SilentlyContinue
    if ($rustupCommand -and -not (Test-ForkCargo $rustupCommand.Source) -and (rustup toolchain list | Select-String '^nightly')) {
        Write-Host 'Adding the Cranelift backend to the nightly toolchain...'
        rustup component add rustc-codegen-cranelift-preview --toolchain nightly *> $null
        if ($LASTEXITCODE -ne 0) { Write-Host 'Cranelift is not available for this nightly; nightly builds use LLVM.' }
    }
    Write-Host 'Note: `rustup self update` reinstalls its proxies; rerun this script afterwards.'
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}
