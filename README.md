# BudgetLaser

Turns a projector in haze into a laser-effect fixture: one virtual DMX fixture that draws
basic laser patterns (dot, line, circle, fan, tunnel) fullscreen. Every lit pixel becomes a
beam from the lens, so the projector behaves like a scanner without moving parts.

Design document: https://claude.ai/artifact/DKb3a19K6z61ffMABD3tJc

## Build and run

```bash
cargo build --release
./target/release/budgetlaser budgetlaser.toml
```

`Esc` quits. All keys in `budgetlaser.toml` are optional; see the comments there.
Log level via `RUST_LOG` (`debug` prints fps every 5 s).

## Input

| Source | Config | Notes |
|---|---|---|
| Art-Net | `source = "artnet"`, `universe` = 15-bit port-address (QLC+ "universe 1" is 0) | Answers ArtPoll; shares UDP 6454 with other apps on the same host |
| sACN | `source = "sacn"`, `universe` = 1..63999 | Multicast; priority ignored in v0.1 |
| DMX USB Pro | `source = "usbpro"`, `serial_port` | Enttec Pro protocol; Open DMX clones cannot receive |

Fixture: 13 channels from `start_address` (mode, pattern, size, rotation, pan, tilt, R, G, B,
dimmer, strobe, beam count, animation speed), as specified in the design document.

Signal loss: blackout after `signal_timeout_s` (default 3 s) or `signal_loss = "hold"`.

## Output

Rust + wgpu: Metal on macOS, DX12/Vulkan on Windows, Vulkan/GLES on Linux and Raspberry Pi.
Vsync (Fifo), additive HDR line rendering with optional glow (`glow_decay` per frame),
`internal_scale = 0.5` renders at half resolution on weak GPUs. `mapping = "fit"` keeps the
square scan field centred; `"stretch"` fills the width.

Platform notes:
- macOS asks once to allow incoming connections for the binary (firewall). Deny = no DMX
  from the network; Art-Net or sACN unicast to 127.0.0.1 still works.
- Windows needs a firewall rule for UDP 6454 / 5568; USB Pro appears as a COM port.

## Raspberry Pi 5

Target: Raspberry Pi OS Lite (trixie), no desktop. BudgetLaser runs fullscreen inside the
`cage` Wayland kiosk on tty1, started by systemd.

1. Get an aarch64 binary: download the `budgetlaser-aarch64-linux` artifact from a CI run,
   or build it on an Apple Silicon Mac in an arm64 container:

   ```bash
   docker run --rm --platform linux/arm64 -v "$PWD":/src -w /src -e CARGO_TARGET_DIR=/src/target/aarch64-linux rust:1.99.1-bookworm cargo build --release --locked
   ```

   The binary lands in `target/aarch64-linux/release/budgetlaser`. It is built against
   glibc 2.36 (bookworm) and runs on both bookworm and trixie.
2. Copy the binary and the `deploy/` directory to the Pi, then run as root:

   ```bash
   sudo deploy/install.sh ./budgetlaser my-config.toml
   ```

   This installs `cage`, creates the `budgetlaser` user (in `dialout` for the USB Pro),
   installs the binary, config, PAM file and `budgetlaser.service`, and starts it. The
   service replaces the login prompt on tty1; use SSH to manage the Pi.
3. Logs: `journalctl -u budgetlaser -f`.

**Measuring the v0.1 gate** (1080p60 with glow, stable): set the beamer to 1080p60, keep
`glow = true` and drive the worst case from another machine for a few minutes:

```bash
tools/dmx_test.py --host <pi-ip> --pattern tunnel --beams 64 --seconds 300
```

Every 10 s the journal shows a line like `60.0 fps, worst frame 16.9 ms, 0 dropped`.
A window with dropped frames logs at warn. The gate passes when every window shows
60 fps and 0 dropped. If it does not, try `internal_scale = 0.75` and then `glow = false`
to see which pass costs the frames.

## Layout

Single crate; modules mirror the planned crates: `point` (point-stream contract), `fixture`
(channel decode, smoothing), `pattern` (generators + transform chain), `input/*` (sources,
triple buffer), `render` (wgpu), `main` (config, window, loop). ILDA sources plug in at the
point stream later.

```bash
cargo test
```
