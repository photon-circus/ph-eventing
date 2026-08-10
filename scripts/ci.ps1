#Requires -Version 5.1
<#
.SYNOPSIS
    Local CI for ph-eventing.

.DESCRIPTION
    Runs the full check matrix locally: formatting, clippy, host tests, docs,
    and the embedded target checks. The GitHub Actions workflow runs on push
    and PR and covers most of this, but not the coverage gate below -- and not
    Miri or Loom, which live in scripts/miri.* and scripts/loom.*. This remains
    the authoritative run.

    Every check runs even if an earlier one fails (matching the workflow's
    fail-fast: false), then a summary is printed. Exit code is non-zero if any
    check failed.

.PARAMETER SkipEmbedded
    Skip the cross-compilation checks. Useful for a quick inner-loop pass when
    the embedded targets are not installed.

.PARAMETER FailFast
    Stop at the first failing check.

.EXAMPLE
    ./scripts/ci.ps1

.EXAMPLE
    ./scripts/ci.ps1 -SkipEmbedded
#>
[CmdletBinding()]
param(
    [switch]$SkipEmbedded,
    [switch]$FailFast,
    [int]$CoverageFloor = 90
)

Set-Location (Join-Path $PSScriptRoot '..')

$checks = @(
    @{ Name = 'fmt'; Args = @('fmt', '--all', '--', '--check') }
    @{ Name = 'clippy'; Args = @('clippy', '--all-targets', '--', '-D', 'warnings') }
    @{ Name = 'test'; Args = @('test') }
    @{ Name = 'doc'; Args = @('doc', '--no-deps'); Env = @{ RUSTDOCFLAGS = '-D warnings' } }

    # Feature combinations, one at a time. `--all-features` cannot work here:
    # the two portable-atomic backend features are mutually exclusive, so the
    # supported set has to be enumerated rather than swept.
    @{ Name = 'features: default'; Args = @('check', '--quiet') }
    @{ Name = 'features: portable-atomic'
       Args = @('check', '--quiet', '--features', 'portable-atomic') }
    @{ Name = 'features: critical-section'
       Args = @('check', '--quiet', '--features', 'portable-atomic-critical-section') }
)

# Current stable. rust-toolchain.toml pins the MSRV, so every other check here
# runs 1.92.0 -- without this, nothing would catch a new stable lint or a
# regression that only appears on a newer compiler. `+stable` is explicit and
# overrides the pin.
$stableInstalled = [bool]((rustup toolchain list 2>$null) -match '^stable')
if ($stableInstalled) {
    $checks += @(
        @{ Name = 'stable: test'; Args = @('+stable', 'test', '--quiet') }
        @{ Name = 'stable: clippy'
           Args = @('+stable', 'clippy', '--all-targets', '--quiet', '--', '-D', 'warnings') }
    )
}

# Supply chain: advisories, licences, bans, sources. Only added when the tool
# is present, since it is a separate install (`cargo install cargo-deny`).
$denyInstalled = [bool](Get-Command cargo-deny -ErrorAction SilentlyContinue)
if ($denyInstalled) {
    $checks += @{ Name = 'deny'; Args = @('deny', 'check') }
}

# Coverage floor. A gate, not a vanity number -- it fails only on a real drop,
# so it cannot rot into decoration. cargo-llvm-cov uses its own target dir, so
# this does not invalidate the normal build cache.
$covInstalled = [bool](Get-Command cargo-llvm-cov -ErrorAction SilentlyContinue)
if ($covInstalled) {
    $checks += @{
        Name = "coverage (>=$CoverageFloor% lines)"
        Args = @('llvm-cov', '--summary-only', '--fail-under-lines', "$CoverageFloor")
    }
}

if (-not $SkipEmbedded) {
    $checks += @(
        @{ Name = 'thumbv6m-none-eabi'
           Args = @('check', '--target', 'thumbv6m-none-eabi',
                    '--features', 'portable-atomic-unsafe-assume-single-core') }
        @{ Name = 'thumbv7em-none-eabi'; Args = @('check', '--target', 'thumbv7em-none-eabi') }
        @{ Name = 'riscv32imac-unknown-none-elf'
           Args = @('check', '--target', 'riscv32imac-unknown-none-elf') }
    )
}

$results = @()

foreach ($check in $checks) {
    Write-Host ''
    Write-Host "==> $($check.Name)" -ForegroundColor Cyan

    $saved = @{}
    if ($check.Env) {
        foreach ($key in $check.Env.Keys) {
            $saved[$key] = [Environment]::GetEnvironmentVariable($key)
            [Environment]::SetEnvironmentVariable($key, $check.Env[$key])
        }
    }

    & cargo @($check.Args)
    $ok = ($LASTEXITCODE -eq 0)

    foreach ($key in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($key, $saved[$key])
    }

    $results += [pscustomobject]@{ Name = $check.Name; Ok = $ok }

    if (-not $ok -and $FailFast) { break }
}

Write-Host ''
Write-Host 'Summary' -ForegroundColor Cyan
if (-not $denyInstalled) {
    Write-Host '  SKIP  deny (not installed: cargo install cargo-deny)' -ForegroundColor Yellow
}
if (-not $covInstalled) {
    Write-Host '  SKIP  coverage (not installed: cargo install cargo-llvm-cov)' -ForegroundColor Yellow
}
if (-not $stableInstalled) {
    Write-Host '  SKIP  stable (not installed: rustup toolchain install stable)' -ForegroundColor Yellow
}
foreach ($result in $results) {
    if ($result.Ok) {
        Write-Host ('  PASS  ' + $result.Name) -ForegroundColor Green
    } else {
        Write-Host ('  FAIL  ' + $result.Name) -ForegroundColor Red
    }
}

$failed = @($results | Where-Object { -not $_.Ok })
if ($failed.Count -gt 0) {
    Write-Host ''
    Write-Host "$($failed.Count) check(s) failed." -ForegroundColor Red
    exit 1
}

Write-Host ''
Write-Host 'All checks passed.' -ForegroundColor Green
exit 0
