#!/usr/bin/env python3
"""Stream to BudgetLaser's IDN receiver without laser software (stdlib only).

Draws a rainbow circle as IDN-RT channel messages on UDP 7255: continuous graphic
mode (wave samples at a sample rate) or discrete mode (whole frames). Needs
[idn] enabled = true.

Examples:
  tools/idn_test.py                                  # wave mode, 30k samples/s
  tools/idn_test.py --mode frame --points 400        # discrete frames at 30 fps
  tools/idn_test.py --host 192.168.1.50 --seconds 30
  tools/idn_test.py --scan                           # list IDN units that answer a broadcast scan
  tools/idn_test.py --scan --host 192.168.1.50       # ask one unit
"""
import argparse
import colorsys
import math
import socket
import struct
import time

PORT = 7255
# X, Y 16 bit; red 638 nm, green 532 nm, blue 460 nm 8 bit; void pads to 32 bit.
TAGS = [0x4200, 0x4010, 0x4210, 0x4010, 0x527E, 0x5214, 0x51CC, 0x0000]
MODES = {"wave": (0x01, 0x01), "frame": (0x02, 0x02)}  # service mode, chunk type


def sample(t: float) -> bytes:
    r, g, b = (int(c * 255) for c in colorsys.hsv_to_rgb(t, 1, 1))
    x, y = 0.8 * math.cos(t * math.tau), 0.8 * math.sin(t * math.tau)
    return struct.pack(">hhBBB", int(x * 32767), int(y * 32767), r, g, b)


def message(mode: str, config: bool, timestamp: int, duration: int, samples: bytes, once: bool = False) -> bytes:
    service_mode, chunk = MODES[mode]
    content = 0x8000 | (0x4000 if config else 0) | chunk  # channel 0
    body = b""
    if config:
        body += struct.pack(">BBBB", len(TAGS) // 2, 0x01, 0, service_mode)  # routing, default service
        body += struct.pack(f">{len(TAGS)}H", *TAGS)
    body += struct.pack(">I", (int(once) << 24) | duration) + samples
    return struct.pack(">HHI", 8 + len(body), content, timestamp & 0xFFFFFFFF) + body


def scan(seconds: float, target: str) -> None:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    s.settimeout(seconds)
    s.sendto(struct.pack(">BBH", 0x10, 0, 1), (target, PORT))
    units = []
    try:
        while True:
            pkt, (ip, _) = s.recvfrom(1500)
            if len(pkt) >= 44 and pkt[0] == 0x11:
                units.append((ip, pkt))
    except socket.timeout:
        pass
    # Ask each unit for its services once the scan replies are in.
    s.settimeout(1.0)
    for ip, pkt in units:
        uid = pkt[8 : 9 + pkt[8]]
        name = pkt[24:44].rstrip(b"\0").decode(errors="replace")
        services = ""
        s.sendto(struct.pack(">BBH", 0x12, 0, 2), (ip, PORT))
        try:
            m = s.recvfrom(1500)[0]
        except socket.timeout:
            m = b""
        if m[:1] == b"\x13":
            entry, count = m[5], m[7]
            for i in range(count):
                e = m[8 + m[6] * entry + i * entry :][:entry]
                services += f" service {e[0]} type {e[1]:#x} {e[4:24].rstrip(bytes(1)).decode(errors='replace')!r}"
        print(f"{ip}  unit {uid[1]:02X}-{uid[2:].hex().upper()}  {name!r} status {pkt[6]:#04x}{services}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--host", help="receiver IP (default: 127.0.0.1; --scan broadcasts)")
    ap.add_argument("--mode", choices=MODES, default="wave")
    ap.add_argument("--rate", type=int, default=30000, help="samples per second (default: 30000)")
    ap.add_argument("--points", type=int, default=600, help="samples per circle (default: 600)")
    ap.add_argument("--seconds", type=float, default=0, help="0 = until Ctrl-C")
    ap.add_argument("--scan", action="store_true", help="broadcast a scan for 3 s and exit")
    a = ap.parse_args()
    if a.scan:
        scan(3.0, a.host or "255.255.255.255")
        return
    a.host = a.host or "127.0.0.1"

    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(1.0)
    seq, i, t0 = 0, 0, time.monotonic()
    last_config = -1.0
    print(f"streaming a {a.points}-point circle in {a.mode} mode at {a.rate} samples/s to {a.host}, Ctrl-C stops")
    try:
        while not a.seconds or time.monotonic() - t0 < a.seconds:
            now = time.monotonic()
            # The configuration rides along every 250 ms, as the spec asks.
            config = now - last_config >= 0.25
            if a.mode == "wave":
                n = 150  # fits one Ethernet frame: continuous streams avoid IP fragmentation
            else:
                n = a.points + 1  # the first sample is the invisible start point
            pts = b"".join(sample(((i + k) % a.points) / a.points) for k in range(n))
            duration = int(n / a.rate * 1e6)
            ts = int((now - t0) * 1e6)
            cmd = 0x41 if seq == 0 else 0x40  # ask for an acknowledge on the first packet
            s.sendto(struct.pack(">BBH", cmd, 0, seq & 0xFFFF) + message(a.mode, config, ts, duration, pts), (a.host, PORT))
            if seq == 0:
                try:
                    ack = s.recvfrom(64)[0]
                except (socket.timeout, ConnectionError):
                    raise SystemExit(f"no answer from {a.host}:{PORT}; is [idn] enabled?")
                if ack[0] != 0x47 or ack[5] != 0:
                    raise SystemExit(f"receiver refused the stream (result {ack[5]:#04x})")
            if config:
                last_config = now
            seq += 1
            if a.mode == "wave":
                i += n
                time.sleep(max(0.0, t0 + i / a.rate - time.monotonic()))
            else:
                time.sleep(1 / 30)
    except KeyboardInterrupt:
        pass
    s.sendto(struct.pack(">BBH", 0x44, 0, seq & 0xFFFF), (a.host, PORT))  # graceful close
    print(f"closed after {seq} messages")


if __name__ == "__main__":
    main()
