#Requires -Version 5.1
<#
.SYNOPSIS
    Local CI for ph-eventing.

.DESCRIPTION
    Runs the full check matrix locally: formatting, clippy, host tests, docs,
    and the embedded target checks. This is the primary CI for this project --
    the GitHub Actions workflow is manual-dispatch only and mirrors these same
    checks.

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
    [switch]$FailFast
)

Set-Location (Join-Path $PSScriptRoot '..')

$checks = @(
    @{ Name = 'fmt'; Args = @('fmt', '--all', '--', '--check') }
    @{ Name = 'clippy'; Args = @('clippy', '--all-targets', '--', '-D', 'warnings') }
    @{ Name = 'test'; Args = @('test') }
    @{ Name = 'doc'; Args = @('doc', '--no-deps'); Env = @{ RUSTDOCFLAGS = '-D warnings' } }
)

# Supply chain: advisories, licences, bans, sources. Only added when the tool
# is present, since it is a separate install (`cargo install cargo-deny`).
$denyInstalled = [bool](Get-Command cargo-deny -ErrorAction SilentlyContinue)
if ($denyInstalled) {
    $checks += @{ Name = 'deny'; Args = @('deny', 'check') }
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
