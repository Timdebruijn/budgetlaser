mod config;
mod fixture;
mod input;
mod pattern;
mod point;
mod render;

use config::{Config, SignalLoss, Source};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

struct App {
    cfg: Config,
    dmx: input::Receiver,
    window: Option<Arc<Window>>,
    renderer: Option<render::Renderer>,
    smoother: fixture::Smoother,
    phases: pattern::Phases,
    frame: point::Frame,
    segments: Vec<point::Segment>,
    start: Instant,
    last_tick: f64,
    last_dmx: Option<f64>,
    fps_count: u32,
    fps_t: f64,
}

impl App {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn tick(&mut self) {
        let now = self.now();
        let dt = (now - self.last_tick).clamp(0.0, 0.1) as f32;
        self.last_tick = now;

        let (fresh, universe) = self.dmx.latest();
        if fresh {
            let a = (self.cfg.input.start_address.clamp(1, 500) - 1) as usize;
            self.smoother
                .push(fixture::decode(&universe[a..a + fixture::CHANNELS]), now);
            if self.last_dmx.is_none() {
                log::info!("DMX signal present");
            }
            self.last_dmx = Some(now);
        }
        let mut params = self.smoother.sample(now);
        let lost = match self.last_dmx {
            Some(t) => now - t > self.cfg.input.signal_timeout_s,
            None => true,
        };
        if lost && self.cfg.input.signal_loss == SignalLoss::Blackout {
            if self
                .last_dmx
                .is_some_and(|t| now - t < self.cfg.input.signal_timeout_s + dt as f64 + 0.001)
            {
                log::warn!("DMX signal lost, blackout");
            }
            params.on = false;
        }
        self.phases.tick(&params, dt);
        pattern::build(
            &params,
            &self.phases,
            self.cfg.output.dimmer_gamma,
            &mut self.frame,
        );
        point::segments(&self.frame.points, &mut self.segments);
        if let Some(r) = &mut self.renderer {
            r.render(&self.segments);
        }

        self.fps_count += 1;
        if now - self.fps_t >= 5.0 {
            log::debug!(
                "{:.1} fps, {} segments",
                self.fps_count as f64 / (now - self.fps_t),
                self.segments.len()
            );
            self.fps_count = 0;
            self.fps_t = now;
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
        let mut attrs = Window::default_attributes().with_title("BudgetLaser");
        if self.cfg.output.fullscreen {
            let m = monitors
                .get(self.cfg.output.monitor)
                .or(monitors.first())
                .cloned();
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(m)));
        } else {
            attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        window.set_cursor_visible(!self.cfg.output.fullscreen);
        self.renderer = Some(render::Renderer::new(
            window.clone(),
            event_loop.owned_display_handle(),
            self.cfg.output.clone(),
        ));
        self.window = Some(window);
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

    let (publisher, receiver) = input::channel();
    let inp = cfg.input.clone();
    std::thread::Builder::new()
        .name("dmx-input".into())
        .spawn(move || {
            let r = match inp.source {
                Source::Artnet => input::artnet::run(inp.universe(), publisher),
                Source::Sacn => input::sacn::run(inp.universe(), publisher),
                Source::Usbpro => input::usbpro::run(&inp.serial_port, publisher),
            };
            if let Err(e) = r {
                log::error!("DMX input stopped: {e}");
            }
        })
        .expect("spawn input thread");

    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        cfg,
        dmx: receiver,
        window: None,
        renderer: None,
        smoother: fixture::Smoother::new(),
        phases: pattern::Phases::default(),
        frame: point::Frame::default(),
        segments: Vec::new(),
        start: Instant::now(),
        last_tick: 0.0,
        last_dmx: None,
        fps_count: 0,
        fps_t: 0.0,
    };
    event_loop.run_app(&mut app).expect("event loop");
}
