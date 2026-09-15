param([Parameter(Mandatory = $true)][string]$Checkout)

$ErrorActionPreference = 'Stop'
$reviewRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
$reviewCheckout = (Resolve-Path -LiteralPath $Checkout).Path
$reviewSource = Join-Path $PSScriptRoot 'probe.rs'
$reviewDestination = Join-Path $reviewCheckout 'crates/oa-core/tests/pr3_review.rs'
$reviewHash = (Get-FileHash -LiteralPath $reviewSource).Hash
$reviewCreated = $false
if (Test-Path -LiteralPath $reviewDestination) {
    if ((Get-FileHash -LiteralPath $reviewDestination).Hash -ne $reviewHash) {
        throw "A different test already exists at $reviewDestination"
    }
} else {
    Copy-Item -LiteralPath $reviewSource -Destination $reviewDestination
    $reviewCreated = $true
}
$reviewOldCapture = $env:RUST_TEST_NOCAPTURE
$reviewOldThreads = $env:RUST_TEST_THREADS
$reviewExit = 1
try {
    $env:RUST_TEST_NOCAPTURE = '1'
    $env:RUST_TEST_THREADS = '1'
    & (Join-Path $reviewRoot 'scripts/cargo.ps1') test `
        --manifest-path (Join-Path $reviewCheckout 'Cargo.toml') `
        --target-dir (Join-Path $reviewCheckout 'target/pr3-review') `
        -p oa-core --test pr3_review --offline
    $reviewExit = $LASTEXITCODE
} finally {
    $env:RUST_TEST_NOCAPTURE = $reviewOldCapture
    $env:RUST_TEST_THREADS = $reviewOldThreads
    if ($reviewCreated -and (Test-Path -LiteralPath $reviewDestination)) {
        if ((Get-FileHash -LiteralPath $reviewDestination).Hash -eq $reviewHash) {
            Remove-Item -LiteralPath $reviewDestination
        } else {
            Write-Warning "Test changed during the run; leaving $reviewDestination in place."
        }
    }
}
exit $reviewExit
