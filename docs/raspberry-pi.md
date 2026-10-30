# Raspberry Pi 5

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
   installs the binary, config, PAM file and `budgetlaser.service`, creates
   `/etc/budgetlaser/ilda` for .ild files, and starts it. The service replaces the login
   prompt on tty1; use SSH to manage the Pi. After adding .ild files, run
   `sudo systemctl restart budgetlaser`.
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
