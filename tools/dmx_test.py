#!/usr/bin/env python3
"""Drive BudgetLaser over Art-Net without a console (stdlib only).

Examples:
  tools/dmx_test.py --pattern tunnel --beams 64            # worst case for the Pi gate
  tools/dmx_test.py --host 192.168.1.50 --pattern fan --seconds 30
  tools/dmx_test.py --pattern ilda --file 0 --color 255,255,255  # file colours, still
"""
import argparse
import math
import socket
import struct
import time

PATTERNS = {
    "dot": 10,
    "line": 30,
    "circle": 50,
    "fan": 70,
    "tunnel": 90,
    "grid": 110,
    "twinkle": 130,
    "wave": 150,
    "polygon": 170,
    "ilda": 190,
}


def artdmx(universe: int, seq: int, data: bytes) -> bytes:
    return (
        b"Art-Net\0"
        + struct.pack("<H", 0x5000)
        + struct.pack(">H", 14)
        + bytes([seq & 0xFF or 1, 0])
        + struct.pack("<H", universe)
        + struct.pack(">H", len(data))
        + data
    )


def dmx_value(v: str) -> int:
    n = int(v)
    if not 0 <= n <= 255:
        raise argparse.ArgumentTypeError(f"{n} is not 0..255")
    return n


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--host", default="127.0.0.1", help="target IP (default: 127.0.0.1)")
    ap.add_argument("--universe", type=int, default=0, help="Art-Net port-address (default: 0)")
    ap.add_argument("--start", type=int, default=1, help="fixture start address (default: 1)")
    ap.add_argument("--pattern", choices=PATTERNS, default="tunnel")
    ap.add_argument("--beams", type=int, default=64, help="1..64; beams, dots, wave cycles or polygon sides per pattern")
    ap.add_argument("--file", type=dmx_value, default=0, help="ILDA file index 0..255 for --pattern ilda")
    ap.add_argument("--color", default="40,255,80", help="R,G,B 0..255")
    ap.add_argument("--seconds", type=float, default=0, help="0 = until Ctrl-C")
    ap.add_argument("--rate", type=float, default=40, help="packets per second (default: 40)")
    a = ap.parse_args()

    r, g, b = (int(v) for v in a.color.split(","))
    beams = round((min(max(a.beams, 1), 64) - 1) / 63 * 255)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    t0, seq = time.monotonic(), 0
    try:
        while a.seconds <= 0 or time.monotonic() - t0 < a.seconds:
            t = time.monotonic() - t0
            ilda = a.pattern == "ilda"  # full size, no rotation or pan sweep; plays at 1x
            universe = bytearray(512)
            ch = [
                255,  # mode: on
                PATTERNS[a.pattern],
                255 if ilda else 200,  # size
                0 if ilda else 150,  # rotation: static / clockwise, slow
                128 if ilda else int(128 + 60 * math.sin(t)),  # pan sweep
                128,  # tilt centre
                r, g, b,
                255,  # dimmer
                0,  # strobe open
                a.file if ilda else beams,
                128 if ilda else 120,  # animation speed (ILDA: 1x frame_rate)
            ]
            universe[a.start - 1 : a.start - 1 + len(ch)] = bytes(ch)
            sock.sendto(artdmx(a.universe, seq, bytes(universe)), (a.host, 6454))
            seq += 1
            time.sleep(1 / a.rate)
    except KeyboardInterrupt:
        pass
    print(f"sent {seq} packets")


if __name__ == "__main__":
    main()
