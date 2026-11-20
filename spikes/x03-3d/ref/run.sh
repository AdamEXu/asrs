#!/bin/bash
# usage: run.sh <www dir> <out dir> [electron switches...]
# Captures the upstream WebGL drawing buffer for both cameras and themes at device scale 1.
set -eu
export DISPLAY=:0 XAUTHORITY=$HOME/.Xauthority TZ=UTC LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8
www=$1
out=$2
shift 2
electron=$(cd "$(dirname "$0")/../../.." && pwd)/target/oracle/upstream/node_modules/electron/dist/electron
mkdir -p "$out"
for cam in field robot; do
  for theme in dark light; do
    "$electron" --force-device-scale-factor=1 --lang=en-US "$@" "$www/main.cjs" \
      --out="$out/upstream_${cam}_${theme}.png" --cam=$cam --theme=$theme 2>&1 | grep -v "ERROR:\|^\[" || true
  done
done
