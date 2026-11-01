#!/usr/bin/env python3
"""Stream to BudgetLaser's Ether Dream DAC without laser software (stdlib only).

Draws a rainbow circle the way laser software does: prepare, keep the DAC buffer
topped up with points, begin at a point rate. Needs [etherdream] enabled = true.

Examples:
  tools/etherdream_test.py                                  # 30k points/s, 600 per frame
  tools/etherdream_test.py --host 192.168.1.50 --rate 20000 --points 400 --seconds 30
  tools/etherdream_test.py --discover                       # list DACs that broadcast
"""
import argparse
import colorsys
import math
import socket
import struct
import time

TCP_PORT = 7765
BROADCAST_PORT = 7654
STATES = {0: "idle", 1: "prepared", 2: "playing"}


def recv_exact(s: socket.socket, n: int) -> bytes:
    b = b""
    while len(b) < n:
        chunk = s.recv(n - len(b))
        if not chunk:
            raise ConnectionError("DAC closed the connection")
        b += chunk
    return b


def status(resp: bytes) -> dict:
    _, le_state, playback, _, _, flags, _, fullness, rate, count = struct.unpack("<BBBBHHHHII", resp[2:22])
    return {"estop": le_state == 3, "playback": playback, "flags": flags, "fullness": fullness, "rate": rate, "count": count}


def command(s: socket.socket, payload: bytes) -> dict:
    s.sendall(payload)
    resp = recv_exact(s, 22)
    if resp[1] != payload[0]:
        raise ConnectionError(f"reply to {chr(resp[1])!r}, expected {chr(payload[0])!r}")
    st = status(resp)
    st["ack"] = resp[0] == ord("a")
    st["resp"] = chr(resp[0])
    return st


def point(x: float, y: float, rgb: tuple) -> bytes:
    r, g, b = (int(c * 65535) for c in rgb)
    return struct.pack("<HhhHHHHHH", 0, int(x * 32767), int(y * 32767), r, g, b, 65535, 0, 0)


def discover(seconds: float) -> None:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    if hasattr(socket, "SO_REUSEPORT"):
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    s.bind(("", BROADCAST_PORT))
    s.settimeout(seconds)
    try:
        while True:
            pkt, (ip, _) = s.recvfrom(64)
            if len(pkt) >= 36:
                mac = ":".join(f"{v:02x}" for v in pkt[:6])
                hw, sw, cap, rate = struct.unpack("<HHHI", pkt[6:16])
                print(f"{ip}  mac {mac}  hw {hw} sw {sw}  buffer {cap}  max {rate} points/s  {STATES.get(pkt[18], pkt[18])}")
    except socket.timeout:
        pass


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--host", default="127.0.0.1", help="DAC IP (default: 127.0.0.1)")
    ap.add_argument("--rate", type=int, default=30000, help="points per second (default: 30000)")
    ap.add_argument("--points", type=int, default=600, help="points per circle (default: 600)")
    ap.add_argument("--seconds", type=float, default=0, help="0 = until Ctrl-C")
    ap.add_argument("--discover", action="store_true", help="listen 3 s for DAC broadcasts and exit")
    a = ap.parse_args()
    if a.discover:
        discover(3.0)
        return

    s = socket.create_connection((a.host, TCP_PORT), timeout=5)
    s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    st = status(recv_exact(s, 22))
    if st["estop"]:
        command(s, b"c")
    if st["playback"] != 0:
        command(s, b"s")
    if not command(s, b"p")["ack"]:
        raise SystemExit("prepare refused")

    # Keep about 50 ms of points buffered, as laser software does.
    target = min(1799, max(100, a.rate // 20))
    i, playing, t0 = 0, False, time.monotonic()
    print(f"streaming a {a.points}-point circle at {a.rate} points/s to {a.host}, Ctrl-C stops")
    try:
        while not a.seconds or time.monotonic() - t0 < a.seconds:
            n = target - st["fullness"]
            if n > 0:
                pts = b""
                for k in range(i, i + n):
                    t = (k % a.points) / a.points
                    pts += point(0.8 * math.cos(t * math.tau), 0.8 * math.sin(t * math.tau), colorsys.hsv_to_rgb(t, 1, 1))
                i += n
                st = command(s, b"d" + struct.pack("<H", n) + pts)
                if not st["ack"]:
                    raise SystemExit(f"data refused ({st['resp']}), DAC {STATES.get(st['playback'])}")
            if not playing:
                st = command(s, b"b" + struct.pack("<HI", 0, a.rate))
                if not st["ack"]:
                    raise SystemExit("begin refused")
                playing = True
            time.sleep(0.01)
            st = command(s, b"?")
            if st["playback"] != 2:
                raise SystemExit(f"DAC stopped playing (flags {st['flags']:#x})")
    except KeyboardInterrupt:
        pass
    command(s, b"s")
    print(f"stopped after sending {i} points")
    s.close()


if __name__ == "__main__":
    main()
