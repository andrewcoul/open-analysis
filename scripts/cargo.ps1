# Uses an existing Rust installation, or the optional repository-local toolchain.
# PowerShell drops a bare "--" from @args, so pass lints via env, e.g.
#   $env:RUSTFLAGS='-D warnings'; .\scripts\cargo.ps1 clippy --all-targets
$solverRoot = Split-Path $PSScriptRoot -Parent
$solverLocalCargo = Join-Path $solverRoot '.tools/cargo/bin/cargo.exe'
if (Test-Path -LiteralPath $solverLocalCargo) {
    $env:CARGO_HOME = Join-Path $solverRoot '.tools/cargo'
    $env:RUSTUP_HOME = Join-Path $solverRoot '.tools/rustup'
    $env:PATH = (Join-Path $solverRoot '.tools/cargo/bin') + ';' + $env:PATH
    $solverToolchain = Join-Path $solverRoot '.tools/rustup/toolchains/stable-x86_64-pc-windows-gnu'
    $solverLinkerDir = Join-Path $solverToolchain 'lib/rustlib/x86_64-pc-windows-gnu/bin/self-contained'
    if (Test-Path -LiteralPath $solverLinkerDir) {
        $env:PATH = $solverLinkerDir + ';' + $env:PATH
        $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $solverLinkerDir 'x86_64-w64-mingw32-gcc.exe'
        $env:PYO3_MINGW_DLLTOOL = Join-Path $solverLinkerDir 'dlltool.exe'
    }
    & $solverLocalCargo @args
} else {
    & cargo @args
}
exit $LASTEXITCODE
