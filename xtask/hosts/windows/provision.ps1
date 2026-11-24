$ErrorActionPreference = 'Stop'

foreach ($tool in 'git', 'cargo', 'rustup', 'tar.exe') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool is not on PATH; install it with the VM's guest\install-toolchain.ps1"
    }
}

# The VM's own Node 26 exits extract-zip early, so Electron's install.js leaves an empty dist.
# xtask puts this Node 24 first on PATH for asrs commands only.
$node = 'node-v24.18.0-win-arm64'
if (-not (Test-Path "C:\tools\$node\node.exe")) {
    $zip = "C:\work\.sync\$node.zip"
    curl.exe -fsSL --retry 5 -o $zip "https://nodejs.org/dist/v24.18.0/$node.zip"
    if ((Get-FileHash -Algorithm SHA256 $zip).Hash -ne 'F274669ADB93B1FD0FBF8F21FD078609E9DCC84333D4F2718D2DDE3F9A161A01') {
        throw "${zip}: sha256 mismatch"
    }
    Expand-Archive -Force $zip C:\tools
    Remove-Item $zip
}

# ring's build script needs clang on aarch64-pc-windows-msvc.
$clang = 'clang-20.1.8'
if (-not (Test-Path "C:\tools\$clang\bin\clang.exe")) {
    $package = 'clang+llvm-20.1.8-aarch64-pc-windows-msvc'
    $archive = "C:\work\.sync\$package.tar.xz"
    curl.exe -fsSL --retry 5 -o $archive "https://github.com/llvm/llvm-project/releases/download/llvmorg-20.1.8/$package.tar.xz".Replace('+', '%2B')
    if ((Get-FileHash -Algorithm SHA256 $archive).Hash -ne '0DF3E81E8FE26370DD2B60B9E009D81CD130D3FDC41B257434AA663C5D9F0C13') {
        throw "${archive}: sha256 mismatch"
    }
    New-Item -Force -ItemType Directory "C:\tools\$clang" | Out-Null
    tar.exe -xf $archive -C "C:\tools\$clang" --strip-components 1 "$package/bin/clang.exe" "$package/lib/clang"
    if ($LASTEXITCODE) { throw "${archive}: tar failed" }
    Remove-Item $archive
}

# Chromium takes ClearType from these, and its gamma and contrast from the tuner's key below if
# there is one.
$desktop = 'HKCU:\Control Panel\Desktop'
Set-ItemProperty $desktop FontSmoothing '2'
Set-ItemProperty $desktop FontSmoothingType 2 -Type DWord
Set-ItemProperty $desktop FontSmoothingOrientation 1 -Type DWord
Set-ItemProperty $desktop FontSmoothingGamma 0 -Type DWord
if (Get-ChildItem HKCU:\Software\Microsoft\Avalon.Graphics -ErrorAction SilentlyContinue) {
    throw 'the ClearType tuner has been run: remove the keys of HKCU\Software\Microsoft\Avalon.Graphics'
}
Set-ItemProperty $desktop LogPixels 96 -Type DWord
Set-ItemProperty $desktop Win8DpiScaling 1 -Type DWord

$personalize = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
if (-not (Test-Path $personalize)) { New-Item $personalize | Out-Null }
Set-ItemProperty $personalize AppsUseLightTheme 1 -Type DWord
Set-ItemProperty $personalize SystemUsesLightTheme 1 -Type DWord

# npm fetches upstream's git dependencies over ssh://git@github.com, and the host has no GitHub key.
git config --global --unset-all url."https://github.com/".insteadOf
git config --global --add url."https://github.com/".insteadOf "ssh://git@github.com/"
git config --global --add url."https://github.com/".insteadOf "git@github.com:"

New-Item -Force -ItemType Directory C:\work\asrs | Out-Null
"provisioned (display scale and theme apply at the next logon)"
