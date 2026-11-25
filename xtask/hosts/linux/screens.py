#!/usr/bin/env python3
"""Gives a running Xvnc the screens it is told, each an output and a CRTC of its own.

screens.py <rfb port> <width>x<height>+<x>+<y>...
"""

import socket
import struct
import sys

EXTENDED_DESKTOP_SIZE = -308


def main():
    port = int(sys.argv[1])
    screens = []
    for text in sys.argv[2:]:
        size, x, y = text.split("+")
        width, height = size.split("x")
        screens.append((int(x), int(y), int(width), int(height)))
    width = max(x + w for x, _, w, _ in screens)
    height = max(y + h for _, y, _, h in screens)

    connection = socket.create_connection(("127.0.0.1", port), timeout=10)
    stream = connection.makefile("rb")
    stream.read(12)
    connection.sendall(b"RFB 003.008\n")
    if 1 not in stream.read(stream.read(1)[0]):
        sys.exit("the server asks for a password")
    connection.sendall(b"\x01")
    if stream.read(4) != bytes(4):
        sys.exit("the server refused")
    connection.sendall(b"\x01")
    stream.read(20)
    stream.read(struct.unpack(">I", stream.read(4))[0])
    connection.sendall(struct.pack(">BxHi", 2, 1, EXTENDED_DESKTOP_SIZE))

    def layouts():
        while True:
            if stream.read(1) != b"\0":
                sys.exit("an unexpected message")
            stream.read(1)
            for _ in range(struct.unpack(">H", stream.read(2))[0]):
                reason, status, w, h, encoding = struct.unpack(">HHHHi", stream.read(12))
                if encoding != EXTENDED_DESKTOP_SIZE:
                    sys.exit(f"an unexpected encoding {encoding}")
                count = stream.read(4)[0]
                boxes = [struct.unpack(">IHHHHI", stream.read(16))[1:5] for _ in range(count)]
                yield reason, status, boxes

    connection.sendall(struct.pack(">BBHHHH", 3, 0, 0, 0, 1, 1))
    updates = layouts()
    next(updates)
    body = struct.pack(">BxHHBx", 251, width, height, len(screens))
    for i, box in enumerate(screens):
        body += struct.pack(">IHHHHI", i + 1, *box, 0)
    connection.sendall(body)
    connection.sendall(struct.pack(">BBHHHH", 3, 1, 0, 0, 1, 1))
    for reason, status, boxes in updates:
        if reason == 1:
            if status != 0:
                sys.exit(f"the server refused the screens: {status}")
            if sorted(boxes) != sorted(screens):
                sys.exit(f"the server has {boxes}")
            return


main()
