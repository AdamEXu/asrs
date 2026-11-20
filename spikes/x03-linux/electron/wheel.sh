#!/bin/bash
# usage: wheel.sh <window title> <command...>
# Creates a uinput wheel mouse, runs the command on :0, scrolls inside its window with XTest
# button clicks and with the uinput device, and prints what the program logged.
set -u
export DISPLAY=:0 XAUTHORITY=$HOME/.Xauthority
here=$(cd "$(dirname "$0")" && pwd)
title=$1
shift
fifo=/tmp/x03-uwheel.fifo
rm -f $fifo && mkfifo $fifo
sudo python3 "$here/uwheel.py" < $fifo > /tmp/x03-uwheel.out 2>&1 &
exec 3> $fifo
sleep 2
: > /tmp/x03-wheel.log
"$@" >> /tmp/x03-wheel.log 2>&1 &
pid=$!
for _ in $(seq 120); do
  win=$( (xdotool search --name "^$title\$"; xdotool search --onlyvisible --pid $pid) 2>/dev/null | head -1)
  [ -n "$win" ] && break
  sleep 0.5
done
sleep 2
eval "$(xdotool getwindowgeometry --shell "$win")"
xdotool windowactivate --sync "$win" 2>/dev/null
xdotool mousemove $((X + WIDTH / 2)) $((Y + HEIGHT / 2))
sleep 0.3
xdotool mousemove_relative 3 3; sleep 0.2; xdotool mousemove_relative -- -3 -3
echo "window $win at $X,$Y ${WIDTH}x$HEIGHT" >> /tmp/x03-wheel.log
sleep 0.5
step() {
  echo "--- $1" >> /tmp/x03-wheel.log
  sleep 0.3
}
step "xtest: one notch down"; xdotool click 5; sleep 0.7
step "xtest: three notches down"; xdotool click --repeat 3 --delay 150 5; sleep 0.7
step "xtest: one notch up"; xdotool click 4; sleep 0.7
step "uinput: one notch down"; echo "-1 1" >&3; sleep 0.9
step "uinput: one notch down again"; echo "-1 1" >&3; sleep 0.9
step "uinput: three notches down"; echo "-1 3" >&3; sleep 1.2
step "uinput: one notch up"; echo "1 1" >&3; sleep 0.9
step "uinput: quarter notches (hi-res 30) x4 down"; echo "-1 4 30" >&3; sleep 1.2
step "end"
echo quit >&3
exec 3>&-
sleep 1
kill $pid 2>/dev/null
wait $pid 2>/dev/null
cat /tmp/x03-wheel.log
