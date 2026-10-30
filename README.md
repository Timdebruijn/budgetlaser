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
| Art-Net | `"artnet"` in `sources`, `artnet_universe` = 15-bit port-address (QLC+ "universe 1" is 0) | Answers ArtPoll; shares UDP 6454 with other apps on the same host |
| sACN | `"sacn"` in `sources`, `sacn_universe` = 1..63999 | Multicast; honours priority, ignores preview data |
| DMX USB Pro | `"usbpro"` in `sources`, `serial_port` | Enttec Pro protocol; Open DMX clones cannot receive |

Several sources and senders can drive the same universe at once. The highest priority among
live senders wins and senders sharing that priority merge HTP (highest value per channel).
Art-Net and USB Pro count as priority 100, the sACN default. A silent sender drops out after
2.5 s (sACN, USB Pro) or 10 s (Art-Net, per its merge rules); an sACN sender that terminates
its stream drops out at once. Per-address priority (start code 0xDD) is not supported.

Fixture: 13 channels (mode, pattern, size, rotation, pan, tilt, R, G, B, dimmer, strobe,
beam count, animation speed), as specified in the design document. Each `[[fixture]]` block
in the config adds one fixture with its own `start_address`, `personality` and `zone` (a
rectangle of the output), so several virtual lasers can share one projector. Footprints may
overlap: two fixtures on the same address mirror each other. All fixtures listen to the same
universe. `personality = "17ch"` adds fine channels 14-17 for size, rotation, pan and tilt; channels
1-13 stay identical, so programming for the 13-channel mode keeps working. A fine channel
is a fraction of one coarse step. Rotation fine only applies to the static-angle range.

**Console fixture definition.** [fixtures/BudgetLaser-Beamer-Laser.qxf](fixtures/BudgetLaser-Beamer-Laser.qxf)
describes both modes (13 and 17 channel) for QLC+ (4.14 and 5.x share the format). Copy it
into the QLC+ user fixture folder and restart QLC+; it then appears under manufacturer
BudgetLaser:

- macOS: `~/Library/Application Support/QLC+/Fixtures`
- Linux: `~/.qlcplus/fixtures`
- Windows: `%UserProfile%\QLC+\Fixtures`

Pan and tilt use QLC+'s position presets, so the EFX editor can move the pattern. The
physical data (30 degrees pan/tilt, 1-30 degrees zoom) is nominal: the real angles depend
on the projector's throw.

Patterns on channel 2. Channel 12 (beams) and channel 13 (animation speed) mean
something per pattern:

| Ch 2 | Pattern | Channel 12 | Animation |
|---|---|---|---|
| 0-19 | Dot | - | - |
| 20-39 | Line | - | sweeps around the centre |
| 40-59 | Circle / cone | - | radius breathes |
| 60-79 | Fan | 1-64 beams | fan opens and closes |
| 80-99 | Tunnel | 1-64 beams | radius breathes |
| 100-119 | Grid | lattice of 1x1 to 8x8 beams | spacing breathes |
| 120-139 | Twinkle | 1-64 dots | dots fade in and out, then move |
| 140-159 | Wave | 1-8 cycles | wave travels |
| 160-179 | Polygon | 3-12 sides | outline breathes |
| 180-255 | reserved | | |

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
2. Copy the binary, `budgetlaser.toml` and the `deploy/` directory to the Pi, then run as
   root (without a config argument the default `budgetlaser.toml` is installed once):

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

## License

Licensed under the [MIT license](LICENSE).
