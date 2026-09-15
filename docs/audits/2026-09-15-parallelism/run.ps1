$ErrorActionPreference = 'Stop'
$auditRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '../../..')).Path
$auditTemporary = Join-Path $auditRoot 'crates/oa-core/tests/parallelism_audit.rs'
if (Test-Path -LiteralPath $auditTemporary) {
    throw "Temporary probe destination already exists: $auditTemporary"
}
$auditSource = Join-Path $PSScriptRoot 'probe.rs'
Copy-Item -LiteralPath $auditSource -Destination $auditTemporary
$auditOriginalHash = (Get-FileHash -LiteralPath $auditTemporary).Hash
Push-Location -LiteralPath $auditRoot
try {
    & cargo test -p oa-core --release --test parallelism_audit --offline parallelism_audit -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Parallelism probe failed: exit $LASTEXITCODE" }
    Write-Output "New measurements: $(Join-Path $auditRoot 'target/parallelism-audit.json')"
} finally {
    Pop-Location
    if ((Test-Path -LiteralPath $auditTemporary) -and
        (Get-FileHash -LiteralPath $auditTemporary).Hash -eq $auditOriginalHash) {
        Remove-Item -LiteralPath $auditTemporary
    } else {
        Write-Warning 'Temporary probe changed during execution; leaving it in place.'
    }
}
