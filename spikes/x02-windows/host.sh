#!/usr/bin/env bash
# X02 driver for the shared Windows VM. Works in C:\work\x02 with its own desktop task, so it never
# touches the C:\work\asrs checkout or the asrs-gui task that `cargo xtask host` uses.
set -euo pipefail

VM=${ASRS_WINDOWS_VM:-/Volumes/T9/vms/win11-arm}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
DIR='C:\work\x02'
GUI='C:\work\.x02gui'
NODE='C:\tools\node-v24.18.0-win-arm64'
STATE="$ROOT/target/hosts/x02"
SSH_OPTS=(-i "$VM/keys/id_ed25519" -o "UserKnownHostsFile=$VM/keys/known_hosts" -o IdentitiesOnly=yes
  -o BatchMode=yes -o LogLevel=ERROR -o ConnectTimeout=10 -o ServerAliveInterval=30)
mkdir -p "$STATE"

ps() {
  local encoded
  encoded=$(printf '%s' "\$ProgressPreference = 'SilentlyContinue'
$1" | iconv -t UTF-16LE | base64)
  ssh "${SSH_OPTS[@]}" -p 2222 dev@127.0.0.1 \
    "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -OutputFormat Text -EncodedCommand $encoded"
}

put() { scp -q "${SSH_OPTS[@]}" -P 2222 "$1" "dev@127.0.0.1:$2"; }
get() { scp -q -r "${SSH_OPTS[@]}" -P 2222 "dev@127.0.0.1:$1" "$2"; }

cmd=${1:-}
shift || true
case "$cmd" in
  ps) ps "$*" ;;
  run)
    ps "\$ErrorActionPreference = 'Continue'
\$env:Path = '$NODE;' + \$env:Path
Set-Location $DIR
& { $* } 2>&1 | Out-String -Stream
exit \$LASTEXITCODE" ;;
  sync)
    (cd "$ROOT" && git ls-files -co --exclude-standard \
      | grep -v -E '^(target|reference)$|(^|/)(target|node_modules)/' \
      | COPYFILE_DISABLE=1 tar -czf "$STATE/sync.tar.gz" -T -)
    ps "New-Item -Force -ItemType Directory $DIR, C:\work\.x02sync | Out-Null"
    put "$STATE/sync.tar.gz" 'C:/work/.x02sync/x02.tar.gz'
    ps "tar.exe -xzf C:/work/.x02sync/x02.tar.gz -C $DIR; exit \$LASTEXITCODE" ;;
  pack)
    put "$ROOT/target/oracle/pack.tar.zst" 'C:/work/.x02sync/pack.tar.zst' ;;
  gui)
    printf '@echo off\r\nset PATH=%s;%%PATH%%\r\ncd /d %s\r\n%s > %s\\last.log 2>&1\r\necho %%ERRORLEVEL%% > %s\\exit.txt\r\n' \
      "$NODE" "$DIR" "$*" "$GUI" "$GUI" > "$STATE/run.cmd"
    ps "New-Item -Force -ItemType Directory $GUI | Out-Null
Remove-Item -Force -ErrorAction SilentlyContinue $GUI\exit.txt, $GUI\last.log
exit 0"
    put "$STATE/run.cmd" 'C:/work/.x02gui/run.cmd'
    ps "\$ErrorActionPreference = 'Stop'
\$action = New-ScheduledTaskAction -Execute '$GUI\run.cmd' -WorkingDirectory '$GUI'
\$principal = New-ScheduledTaskPrincipal -UserId dev -LogonType Interactive -RunLevel Highest
\$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Hours 4) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName x02-gui -Action \$action -Principal \$principal -Settings \$settings -Force | Out-Null
Start-ScheduledTask -TaskName x02-gui
\$idle = 0
while (-not (Test-Path $GUI\exit.txt)) {
  Start-Sleep -Milliseconds 500
  if ((Get-ScheduledTask -TaskName x02-gui).State -eq 'Ready') { \$idle++ } else { \$idle = 0 }
  if (\$idle -ge 20 -and -not (Test-Path $GUI\exit.txt)) { Get-Content -ErrorAction SilentlyContinue $GUI\last.log; exit 1 }
}
Start-Sleep -Milliseconds 200
Get-Content $GUI\last.log
exit [int](Get-Content $GUI\exit.txt).Trim()" ;;
  fetch)
    ps "tar.exe -czf C:/work/.x02sync/fetch.tar.gz -C $DIR/target $1; exit \$LASTEXITCODE"
    mkdir -p "$ROOT/target/x02-win"
    get 'C:/work/.x02sync/fetch.tar.gz' "$STATE/fetch.tar.gz"
    tar -xzf "$STATE/fetch.tar.gz" -C "$ROOT/target/x02-win" ;;
  get) get "$1" "$2" ;;
  put) put "$1" "$2" ;;
  screenshot) "$VM/vm.sh" screenshot "$1" ;;
  *) echo "usage: host.sh ps|run|sync|pack|gui|fetch|get|put|screenshot ..." >&2; exit 2 ;;
esac
