#Requires -Version 5.1
<#
.SYNOPSIS
    Miri (UB + concurrency) checks for ph-eventing.

.DESCRIPTION
    Runs the test suite under Miri, which interprets MIR and models the C++20
    weak-memory semantics. It catches undefined behaviour, data races, and
    ordering bugs that a native run on strongly-ordered x86 will never expose.

    Two passes are required, because SeqRing's seqlock has a known and
    unavoidable formal data race (see the SeqRing module docs):

      1. Full checking, skipping the SeqRing overwrite stress test.
      2. The SeqRing overwrite test alone, with the data-race detector off, so
         its logic (no stale or torn payload, exact drop accounting) is still
         verified under Miri's scheduler.

    Cross-target passes catch pointer-width and endianness bugs. The bare-metal
    targets cannot be run: Miri needs `std` for the test harness, and no_std
    targets have none. The 32-bit std targets stand in for them -- they share
    the pointer width and `usize` range that actually matter here.

.PARAMETER Seeds
    Number of scheduler seeds to explore per concurrency test. Higher finds
    more interleavings and takes proportionally longer.

.PARAMETER HostOnly
    Skip the cross-target passes.

.EXAMPLE
    ./scripts/miri.ps1

.EXAMPLE
    ./scripts/miri.ps1 -Seeds 64
#>
[CmdletBinding()]
param(
    [int]$Seeds = 16,
    [switch]$HostOnly
)

Set-Location (Join-Path $PSScriptRoot '..')

# Proxies for the bare-metal targets: 32-bit pointers (i686, armv7) and
# big-endian (s390x). thumbv*/riscv32 themselves cannot host a test harness.
$crossTargets = @(
    'i686-unknown-linux-gnu'
    'armv7-unknown-linux-gnueabihf'
    's390x-unknown-linux-gnu'
)

$base = '-Zmiri-disable-isolation'
$skipSeqlock = @('--', '--skip', 'concurrent_overwrite_never_yields_a_mismatched_value')

$results = @()

function Invoke-Miri {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Flags,
        [Parameter(Mandatory)][string[]]$CargoArgs
    )

    Write-Host ''
    Write-Host "==> $Name" -ForegroundColor Cyan

    $previous = $env:MIRIFLAGS
    $env:MIRIFLAGS = $Flags
    & cargo +nightly miri test @CargoArgs
    $ok = ($LASTEXITCODE -eq 0)
    $env:MIRIFLAGS = $previous

    $script:results += [pscustomobject]@{ Name = $Name; Ok = $ok }
}

Invoke-Miri -Name 'host: full checking' -Flags $base -CargoArgs $skipSeqlock

Invoke-Miri -Name 'host: seqlock logic (race detector off)' `
    -Flags "$base -Zmiri-disable-data-race-detector" `
    -CargoArgs @('concurrent_overwrite_never_yields_a_mismatched_value')

Invoke-Miri -Name "host: $Seeds scheduler seeds" `
    -Flags "$base -Zmiri-many-seeds=0..$Seeds" `
    -CargoArgs @('--', 'concurrent_spsc', 'len_stays')

if (-not $HostOnly) {
    foreach ($target in $crossTargets) {
        Invoke-Miri -Name "$target" -Flags $base `
            -CargoArgs (@('--target', $target) + $skipSeqlock)
    }
}

Write-Host ''
Write-Host 'Summary' -ForegroundColor Cyan
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
    Write-Host "$($failed.Count) Miri pass(es) failed." -ForegroundColor Red
    exit 1
}

Write-Host ''
Write-Host 'All Miri passes clean.' -ForegroundColor Green
exit 0
