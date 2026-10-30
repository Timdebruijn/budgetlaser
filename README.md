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
- macOS asks once to allow incoming connections for the binary (firewall). Deny = no DMX.
- Windows needs a firewall rule for UDP 6454 / 5568; USB Pro appears as a COM port.
- Raspberry Pi OS Lite: run under `cage` as a systemd service, add the user to `dialout`
  for the USB Pro. Cross-compile with `cross build --release --target aarch64-unknown-linux-gnu`.

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
