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

$desktop = 'HKCU:\Control Panel\Desktop'
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
