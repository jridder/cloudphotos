# Development setup on Windows: the helper's Python with pyicloud, in a
# venv where the app looks for it (%LOCALAPPDATA%\cloudphotos\venv).
# Needs MSYS2 with the packages listed in the README (Windows).
. "$PSScriptRoot\env.ps1"
# Maturin (building any Rust-based wheel) must not use the app's target dir.
Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
# pyicloud's compiled dependencies come from MSYS2 (PyPI's Windows wheels
# don't fit its Python); pip adds the pure-Python rest.
& "$env:MSYS2_ROOT_DIR\usr\bin\pacman.exe" -S --needed --noconfirm `
    mingw-w64-ucrt-x86_64-python-cryptography mingw-w64-ucrt-x86_64-python-cffi `
    mingw-w64-ucrt-x86_64-python-pydantic mingw-w64-ucrt-x86_64-python-requests `
    mingw-w64-ucrt-x86_64-python-keyring mingw-w64-ucrt-x86_64-python-yaml
if (-not $?) { throw 'Installing Python packages from MSYS2 failed.' }
$venv = Join-Path $env:LOCALAPPDATA 'cloudphotos\venv'
if (-not (Test-Path "$venv\bin\python.exe")) {
    python -m venv --system-site-packages $venv
    if (-not $?) { throw 'Creating the venv failed.' }
}
& "$venv\bin\python.exe" -m pip install --upgrade -r (Join-Path $PSScriptRoot '..\helper\requirements.txt')
if (-not $?) { throw 'Installing pyicloud failed.' }
Write-Host "Helper Python ready: $venv\bin\python.exe"
