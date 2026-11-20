$ErrorActionPreference = 'Stop'

foreach ($tool in 'git', 'cargo', 'rustup', 'node', 'npm.cmd', 'tar.exe') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        throw "$tool is not on PATH; install it with the VM's guest\install-toolchain.ps1"
    }
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
