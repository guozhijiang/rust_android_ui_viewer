#!/usr/bin/env pwsh
# check-coverage.ps1
#
# Run the unit/integration tests under source-based coverage (line + region),
# enforce the coverage gate (lines >= 90%, regions >= 85%), and emit a
# browsable HTML report plus an lcov file under ./coverage.
#
# Device/GUI modules (app, live, scrcpy, adb, u2, main, lib) are excluded from
# the gate because they require a live device or a running egui window and are
# covered by integration tests instead.

$ErrorActionPreference = 'Continue'

$Root = Resolve-Path (Join-Path $PSScriptRoot '..')
$CovDir = Join-Path $Root 'coverage'
New-Item -ItemType Directory -Force -Path $CovDir | Out-Null

# Modules that need a device / GUI context and are not unit-testable in CI.
$Ignore = 'src[\\/](app|live|scrcpy|adb|u2|main|lib)\.rs'

cargo llvm-cov clean --workspace

Write-Host "[coverage] Running tests with coverage (lines >= 90%, regions >= 85%)..."
cargo llvm-cov test --workspace `
    --ignore-filename-regex $Ignore `
    --lcov --output-path (Join-Path $CovDir 'lcov.info') `
    --fail-under-lines 90 `
    --fail-under-regions 85
$gate = $LASTEXITCODE

# Render a browsable HTML report (does not affect the gate result).
# llvm-cov writes it under <output-dir>/html, so pass the coverage dir itself.
cargo llvm-cov report --html --output-dir $CovDir | Out-Null

if ($gate -ne 0) {
    Write-Error "[coverage] GATE FAILED (lines >= 90%, regions >= 85%). See $CovDir\html\index.html"
} else {
    Write-Host "[coverage] Gate passed. Report: $CovDir\html\index.html"
}
exit $gate
