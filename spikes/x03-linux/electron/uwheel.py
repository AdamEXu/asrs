import fcntl
import os
import struct
import sys
import time

UI_SET_EVBIT, UI_SET_KEYBIT, UI_SET_RELBIT = 0x40045564, 0x40045565, 0x40045566
UI_DEV_CREATE, UI_DEV_DESTROY = 0x5501, 0x5502
EV_SYN, EV_KEY, EV_REL = 0, 1, 2
BTN_LEFT, REL_X, REL_Y, REL_WHEEL, REL_WHEEL_HI_RES = 0x110, 0, 1, 8, 11

fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
fcntl.ioctl(fd, UI_SET_KEYBIT, BTN_LEFT)
fcntl.ioctl(fd, UI_SET_EVBIT, EV_REL)
for code in (REL_X, REL_Y, REL_WHEEL, REL_WHEEL_HI_RES):
    fcntl.ioctl(fd, UI_SET_RELBIT, code)
name = b"x03 wheel mouse".ljust(80, b"\0")
os.write(fd, name + struct.pack("HHHHi", 3, 0x1234, 0x5678, 1, 0) + b"\0" * (4 * 64 * 4))
fcntl.ioctl(fd, UI_DEV_CREATE)


def emit(t, c, v):
    sec = time.time()
    os.write(fd, struct.pack("llHHi", int(sec), int((sec % 1) * 1e6), t, c, v))


print("ready", flush=True)
for line in sys.stdin:
    parts = line.split()
    if not parts:
        continue
    if parts[0] == "quit":
        break
    direction, count = int(parts[0]), int(parts[1])
    step = int(parts[2]) if len(parts) > 2 else 120
    acc = 0
    for _ in range(count):
        acc += step
        emit(EV_REL, REL_WHEEL_HI_RES, direction * step)
        if acc >= 120:
            emit(EV_REL, REL_WHEEL, direction * (acc // 120))
            acc %= 120
        emit(EV_SYN, 0, 0)
        time.sleep(0.15)
    print("done", flush=True)
fcntl.ioctl(fd, UI_DEV_DESTROY)
os.close(fd)
