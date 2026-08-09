#Requires -Version 5.1
<#
.SYNOPSIS
    Loom model checking for ph-eventing.

.DESCRIPTION
    Loom enumerates every legal execution of a model -- every thread
    interleaving, and every value each relaxed load may return under the C11
    memory model. Where Miri samples schedules, Loom is exhaustive: a clean run
    proves the absence of ordering bugs for the modelled size.

    Loom is a dev-dependency gated on `--cfg loom`, so it is only resolved and
    downloaded by this script. A normal build, test, or package never sees it
    and the library keeps zero runtime dependencies.

    Models live in src/loom_tests.rs. They are deliberately tiny (capacity 2,
    two or three operations per thread) because the state space grows
    exponentially.

.PARAMETER Filter
    Only run models whose name contains this string.

.PARAMETER MaxPreemptions
    Loom's preemption bound. Higher explores more interleavings at exponential
    cost.

.EXAMPLE
    ./scripts/loom.ps1

.EXAMPLE
    ./scripts/loom.ps1 -Filter event_buf -MaxPreemptions 3
#>
[CmdletBinding()]
param(
    [string]$Filter = '',
    [int]$MaxPreemptions = 2
)

Set-Location (Join-Path $PSScriptRoot '..')

Write-Host "==> loom (max_preemptions=$MaxPreemptions)" -ForegroundColor Cyan

$savedFlags = $env:RUSTFLAGS
$savedPreempt = $env:LOOM_MAX_PREEMPTIONS
$env:RUSTFLAGS = '--cfg loom'
$env:LOOM_MAX_PREEMPTIONS = "$MaxPreemptions"

$testArgs = @('test', '--lib', 'loom_tests')
if ($Filter) { $testArgs += $Filter }

& cargo @testArgs
$ok = ($LASTEXITCODE -eq 0)

$env:RUSTFLAGS = $savedFlags
$env:LOOM_MAX_PREEMPTIONS = $savedPreempt

Write-Host ''
if ($ok) {
    Write-Host 'All Loom models verified.' -ForegroundColor Green
    exit 0
}

Write-Host 'Loom found a failing execution. The output above replays the' -ForegroundColor Red
Write-Host 'exact interleaving -- it is deterministic, so re-running reproduces it.' -ForegroundColor Red
exit 1
