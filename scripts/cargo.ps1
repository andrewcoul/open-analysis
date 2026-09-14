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
    # rustup's gcc is a linker only. C dependencies such as bundled SQLite need a
    # real compiler; use WinLibs MinGW-w64 (winget: BrechtSanders.WinLibs.POSIX.MSVCRT) if present.
    $solverMingw = Get-ChildItem -Path (Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages') -Directory -Filter 'BrechtSanders.WinLibs.POSIX.MSVCRT*' -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($solverMingw) {
        $solverMingwBin = Join-Path $solverMingw.FullName 'mingw64\bin'
        if (Test-Path -LiteralPath (Join-Path $solverMingwBin 'gcc.exe')) {
            # Link with the full toolchain too: rustup's bundled dlltool has no
            # assembler, so raw-dylib imports (getrandom, tokio) fail to link.
            $env:PATH = $solverMingwBin + ';' + $env:PATH
            $env:CC = Join-Path $solverMingwBin 'gcc.exe'
            $env:AR = Join-Path $solverMingwBin 'ar.exe'
            $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = Join-Path $solverMingwBin 'gcc.exe'
            $env:CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = '-C link-self-contained=no'
            $env:PYO3_MINGW_DLLTOOL = Join-Path $solverMingwBin 'dlltool.exe'
        }
    }
    & $solverLocalCargo @args
} else {
    & cargo @args
}
exit $LASTEXITCODE
