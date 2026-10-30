mod config;
mod fixture;
mod ilda;
mod input;
mod pattern;
mod point;
mod render;

use config::{Config, SignalLoss, Source};
use fixture::Pattern;
use std::sync::Arc;
use std::time::Instant;
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

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "budgetlaser.toml".into());
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

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
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
    event_loop.run_app(&mut app).expect("event loop");
}

#[cfg(test)]
mod tests {
    use super::FrameStats;

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
