mod config;
mod fixture;
mod ilda;
mod input;
mod pattern;
mod point;
mod render;

use config::{Config, SignalLoss, Source};
use fixture::Pattern;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

/// Per-fixture render state.
struct Fixture {
    cfg: config::Fixture,
    smoother: fixture::Smoother,
    phases: pattern::Phases,
}

struct App {
    cfg: Config,
    dmx: input::Receiver,
    window: Option<Arc<Window>>,
    renderer: Option<render::Renderer>,
    fixtures: Vec<Fixture>,
    clips: Vec<ilda::Clip>,
    frame: point::Frame,
    segments: Vec<point::Segment>,
    scratch: Vec<point::Segment>,
    batches: Vec<render::Batch>,
    start: Instant,
    last_tick: f64,
    last_dmx: Option<f64>,
    signal_ok: bool,
    stats: FrameStats,
}

/// Frame pacing per 10 s window. With Fifo present every frame should take one
/// refresh interval; a frame taking 1.5x that or more means the display showed
/// the previous image twice (a drop). The Pi gate is measured on this.
struct FrameStats {
    interval: f64,
    t0: f64,
    frames: u32,
    late: u32,
    worst: f64,
}

impl FrameStats {
    const WINDOW_S: f64 = 10.0;

    fn new(refresh_hz: f64) -> Self {
        Self {
            interval: 1.0 / refresh_hz,
            t0: 0.0,
            frames: 0,
            late: 0,
            worst: 0.0,
        }
    }

    /// Records one frame; returns (fps, worst frame ms, late frames) when a window closes.
    fn push(&mut self, now: f64, dt: f64) -> Option<(f64, f64, u32)> {
        if now < self.t0 {
            return None; // warm-up
        }
        self.frames += 1;
        self.worst = self.worst.max(dt);
        if dt > 1.5 * self.interval {
            self.late += 1;
        }
        if now - self.t0 < Self::WINDOW_S {
            return None;
        }
        let report = (
            self.frames as f64 / (now - self.t0),
            self.worst * 1000.0,
            self.late,
        );
        *self = Self {
            t0: now,
            ..Self::new(1.0 / self.interval)
        };
        Some(report)
    }
}

impl App {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn tick(&mut self) {
        let now = self.now();
        let frame_dt = now - self.last_tick;
        let dt = frame_dt.clamp(0.0, 0.1) as f32;
        self.last_tick = now;

        let (fresh, universe) = self.dmx.latest();
        if fresh {
            for f in &mut self.fixtures {
                let a = f.cfg.start_address as usize - 1;
                let n = f.cfg.personality.footprint();
                f.smoother.push(fixture::decode(&universe[a..a + n]), now);
            }
            self.last_dmx = Some(now);
        }
        let ok = self
            .last_dmx
            .is_some_and(|t| now - t <= self.cfg.input.signal_timeout_s);
        if ok != self.signal_ok {
            self.signal_ok = ok;
            match (ok, self.cfg.input.signal_loss) {
                (true, _) => log::info!("DMX signal present"),
                (false, SignalLoss::Blackout) => log::warn!("DMX signal lost, blackout"),
                (false, SignalLoss::Hold) => log::warn!("DMX signal lost, holding last state"),
            }
        }
        let blackout = !ok && self.cfg.input.signal_loss == SignalLoss::Blackout;

        self.segments.clear();
        self.batches.clear();
        for f in &mut self.fixtures {
            let mut params = f.smoother.sample(now);
            params.on &= !blackout;
            f.phases.tick(&params, dt);
            let clip = match params.pattern {
                Some(Pattern::Ilda) => self.clips.get(params.file as usize),
                _ => None,
            }
            .map_or(&[][..], Vec::as_slice);
            f.phases
                .tick_clip(clip.len(), &params, self.cfg.ilda.frame_rate, dt);
            pattern::build(
                &params,
                &f.phases,
                clip.get(f.phases.clip_pos as usize)
                    .map_or(&[], Vec::as_slice),
                self.cfg.output.dimmer_gamma,
                &mut self.frame,
            );
            point::segments(&self.frame.points, &mut self.scratch);
            let start = self.segments.len();
            let width = params.width;
            self.segments
                .extend(self.scratch.iter().map(|s| point::Segment { width, ..*s }));
            self.batches.push(render::Batch {
                range: start..self.segments.len(),
                zone: f.cfg.zone,
            });
        }
        let presented = self
            .renderer
            .as_mut()
            .is_some_and(|r| r.render(&self.segments, &self.batches));

        // Hidden frames sleep instead of waiting for vsync; they say nothing about pacing.
        if !presented {
            return;
        }

        if let Some((fps, worst_ms, late)) = self.stats.push(now, frame_dt) {
            let level = if late > 0 {
                log::Level::Warn
            } else {
                log::Level::Debug
            };
            log::log!(
                level,
                "{fps:.1} fps, worst frame {worst_ms:.1} ms, {late} dropped, {} segments",
                self.segments.len()
            );
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let monitors: Vec<_> = event_loop.available_monitors().collect();
        for (i, m) in monitors.iter().enumerate() {
            log::info!(
                "monitor {i}: {} {}x{} @ {} mHz",
                m.name().unwrap_or_default(),
                m.size().width,
                m.size().height,
                m.refresh_rate_millihertz().unwrap_or(0)
            );
        }
        let monitor = monitors
            .get(self.cfg.output.monitor)
            .or(monitors.first())
            .cloned();
        let mut attrs = Window::default_attributes().with_title("BudgetLaser");
        if self.cfg.output.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(monitor.clone())));
        } else {
            attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        window.set_cursor_visible(!self.cfg.output.fullscreen);
        // Wayland only reports current_monitor() after the surface enters an output,
        // so use the monitor chosen for fullscreen.
        let refresh_hz = monitor
            .and_then(|m| m.refresh_rate_millihertz())
            .map_or(60.0, |mhz| mhz as f64 / 1000.0);
        log::info!("display refresh {refresh_hz:.2} Hz");
        self.stats = FrameStats::new(refresh_hz);
        self.renderer = Some(render::Renderer::new(
            window.clone(),
            event_loop.owned_display_handle(),
            self.cfg.output.clone(),
        ));
        self.window = Some(window);
        // Start measuring 1 s after window and GPU setup: the first frames include
        // pipeline warm-up and would count as drops.
        self.last_tick = self.now();
        self.stats.t0 = self.last_tick + 1.0;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape) =>
            {
                event_loop.exit()
            }
            WindowEvent::Resized(size) => {
                if let Some(r) = &mut self.renderer {
                    r.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => self.tick(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Fifo present blocks in render(), so this paces the loop at the display refresh.
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

const USAGE: &str = "usage: budgetlaser [config.toml] [--record DIR [--seconds S] [--size WxH]]";

/// Renders offscreen at 60 fps for `seconds` and writes every frame to `dir` as a
/// BMP, for screenshots and demo GIFs without a display. DMX input works as usual.
fn record(mut app: App, dir: &Path, seconds: f64, (w, h): (u32, u32)) {
    // Glow decays per frame and is tuned for a 60 Hz display.
    const FPS: f64 = 60.0;
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("{}: {e}", dir.display());
        std::process::exit(2);
    }
    app.renderer = Some(render::Renderer::offscreen(w, h, app.cfg.output.clone()));
    app.stats = FrameStats::new(FPS);
    app.last_tick = app.now();
    let t0 = Instant::now();
    let frames = ((seconds * FPS).round() as u32).max(1);
    log::info!("recording {frames} frames of {w}x{h} to {}", dir.display());
    for i in 0..frames {
        // Pace in real time, so DMX sent alongside lines up with the frames.
        let due = t0 + Duration::from_secs_f64(i as f64 / FPS);
        let now = Instant::now();
        if now > due + Duration::from_secs_f64(1.0 / FPS) {
            // Animation follows the wall clock, so a slow machine records a fast clip.
            log::warn!(
                "frame {i} is {:.0} ms late",
                (now - due).as_secs_f64() * 1000.0
            );
        }
        std::thread::sleep(due.saturating_duration_since(now));
        app.tick();
        let (fw, fh, rgba) = app.renderer.as_ref().unwrap().read_frame();
        let file = dir.join(format!("frame_{i:05}.bmp"));
        if let Err(e) = std::fs::write(&file, bmp(fw, fh, &rgba)) {
            eprintln!("{}: {e}", file.display());
            std::process::exit(1);
        }
    }
}

/// 24-bit BMP: no image library needed, and every image tool reads it.
fn bmp(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut out = b"BM".to_vec();
    for v in [size, 0, 54, 40, w, h] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
    for v in [0, row * h, 2835, 2835, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes()); // uncompressed, 72 dpi
    }
    for y in (0..h as usize).rev() {
        // Bottom row first, BGR.
        for px in rgba[y * w as usize * 4..][..w as usize * 4].chunks(4) {
            out.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        out.resize(out.len() + (row - w * 3) as usize, 0);
    }
    out
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut path = "budgetlaser.toml".to_string();
    let (mut record_dir, mut seconds, mut size) = (None, 10.0, (960, 540));
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let parsed = match arg.as_str() {
            "--record" => args
                .next()
                .filter(|d| !d.starts_with('-'))
                .map(|d| record_dir = Some(d)),
            "--seconds" => args
                .next()
                .and_then(|v| v.parse().ok())
                .filter(|s: &f64| s.is_finite() && *s > 0.0)
                .map(|s| seconds = s),
            "--size" => args
                .next()
                .and_then(|v| {
                    let (w, h) = v.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
                .filter(|&(w, h): &(u32, u32)| w > 0 && h > 0)
                .map(|s| size = s),
            a if a.starts_with('-') => None,
            _ => {
                path = arg.clone();
                Some(())
            }
        };
        if parsed.is_none() {
            eprintln!("bad argument {arg}\n{USAGE}");
            std::process::exit(2);
        }
    }
    let cfg = match config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            std::process::exit(2);
        }
    };
    log::info!("config: {cfg:?}");
    let ilda_dir = std::path::Path::new(&path)
        .parent()
        .unwrap_or(std::path::Path::new(""))
        .join(&cfg.ilda.dir);
    let clips = ilda::load_dir(&ilda_dir);

    let (sink, receiver) = input::channel();
    for &source in &cfg.input.sources {
        let (inp, sink) = (cfg.input.clone(), sink.clone());
        std::thread::Builder::new()
            .name(format!("dmx-{source:?}").to_lowercase())
            .spawn(move || {
                let r = match source {
                    Source::Artnet => input::artnet::run(inp.artnet_universe, sink),
                    Source::Sacn => input::sacn::run(inp.sacn_universe, sink),
                    Source::Usbpro => input::usbpro::run(&inp.serial_port, sink),
                };
                if let Err(e) = r {
                    log::error!("{source:?} input stopped: {e}");
                }
            })
            .expect("spawn input thread");
    }
    std::thread::Builder::new()
        .name("dmx-tick".into())
        .spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                sink.tick();
            }
        })
        .expect("spawn input ticker");

    let mut app = App {
        cfg: cfg.clone(),
        dmx: receiver,
        window: None,
        renderer: None,
        fixtures: cfg
            .fixtures
            .iter()
            .map(|f| Fixture {
                cfg: f.clone(),
                smoother: fixture::Smoother::new(),
                phases: pattern::Phases::default(),
            })
            .collect(),
        clips,
        frame: point::Frame::default(),
        segments: Vec::new(),
        scratch: Vec::new(),
        batches: Vec::new(),
        start: Instant::now(),
        last_tick: 0.0,
        last_dmx: None,
        signal_ok: false,
        stats: FrameStats::new(60.0),
    };
    if let Some(dir) = record_dir {
        record(app, Path::new(&dir), seconds, size);
        return;
    }
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut app).expect("event loop");
}

#[cfg(test)]
mod tests {
    use super::{FrameStats, bmp};

    #[test]
    fn bmp_layout() {
        // 2x2 RGBA, top row red and green, bottom row blue and white.
        let px = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let b = bmp(2, 2, &px);
        assert_eq!(&b[..2], b"BM");
        assert_eq!(b.len(), 54 + 2 * 8); // rows of 6 bytes padded to 8
        assert_eq!(
            u32::from_le_bytes(b[2..6].try_into().unwrap()),
            b.len() as u32
        );
        // Bottom row first, BGR: blue, white, padding; then red, green.
        assert_eq!(&b[54..62], &[255, 0, 0, 255, 255, 255, 0, 0]);
        assert_eq!(&b[62..68], &[0, 0, 255, 0, 255, 0]);
    }

    #[test]
    fn frame_stats_counts_drops() {
        let mut s = FrameStats::new(60.0);
        let mut t = 0.0;
        let mut report = None;
        for i in 0..700 {
            let dt = if i == 100 { 2.0 / 60.0 } else { 1.0 / 60.0 };
            t += dt;
            report = report.or(s.push(t, dt));
        }
        let (fps, worst_ms, late) = report.expect("window closed after 10 s");
        assert_eq!(late, 1);
        assert!((worst_ms - 33.3).abs() < 0.1);
        assert!((fps - 60.0).abs() < 0.5, "{fps}");
    }
}
