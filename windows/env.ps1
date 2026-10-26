# Put MSYS2's UCRT64 environment (GTK, libadwaita, Rust, pkg-config) first
# on PATH for this PowerShell session. Dot-source it: . windows\env.ps1
$msys = if ($env:MSYS2_ROOT) { $env:MSYS2_ROOT } else { 'C:\msys64' }
if (-not (Test-Path "$msys\ucrt64\bin\cargo.exe")) {
    throw "MSYS2 UCRT64 with Rust not found under $msys; see README (Windows)."
}
$env:MSYS2_ROOT_DIR = $msys
$env:PATH = "$msys\ucrt64\bin;$msys\usr\bin;$env:PATH"
$env:PKG_CONFIG_PATH = "$msys\ucrt64\lib\pkgconfig"
# Cargo builds into its own target dir so it never mixes with an MSVC build.
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot '..\target\ucrt64'
