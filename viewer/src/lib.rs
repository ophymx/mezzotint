//! A desktop window onto mezzotint's simulated e-ink panel, for driving a
//! shell with the mouse and watching what the glass does.
//!
//! [`SimulatedPanel`] keeps updates in flight on a clock its caller
//! advances. This crate makes the caller a frame loop and the clock wall
//! time, so a page turn lands after its latency and a drag's monochrome
//! strips after theirs, and puts the result in a window with two overlays
//! a real device cannot offer: the regions the controller is still
//! reading, outlined, and a flash painted inverted for its first beat.
//! The title bar carries the fault count — writes under a live update,
//! which is the rule [`PanelDriver`] exists to hold, and here a number to
//! watch stay at zero.
//!
//! # Shape
//!
//! A shell is written against `PanelDriver<P>` generically; that is what
//! lets the same code run on a device and in here. This crate supplies
//! the panel and the loop, and calls the shell once per frame:
//!
//! ```no_run
//! use mezzotint::{PanelDriver, SimulatedPanel, Source, UpdateClass};
//! use mezzotint_viewer::{App, Config, Control, Event, Input, Key};
//!
//! struct Shell { page: Vec<u8>, dirty: bool }
//!
//! impl App for Shell {
//!     fn frame(&mut self, driver: &mut PanelDriver<SimulatedPanel>, input: &Input) -> Control {
//!         for event in &input.events {
//!             match event {
//!                 Event::Key(Key::Escape) => return Control::Exit,
//!                 Event::Key(Key::Space) => self.dirty = true,
//!                 _ => {}
//!             }
//!         }
//!         if self.dirty {
//!             let info = driver.info();
//!             let src = Source::panel(&self.page, info.width, info.height).unwrap();
//!             driver.present(src, None, UpdateClass::Quality).unwrap();
//!             self.dirty = false;
//!         }
//!         Control::Continue
//!     }
//! }
//!
//! let config = Config::default();
//! let page = vec![0xFF; config.buffer_len()];
//! mezzotint_viewer::run(config, Shell { page, dirty: true }).unwrap();
//! ```
//!
//! The window's event loop has to own the main thread, which is why the
//! shell runs inside [`App::frame`] rather than driving a loop of its
//! own. Before each call the panel's clock has advanced by the wall time
//! since the last frame and every update that finished has been retired,
//! so the driver's view is current.
//!
//! # Not blocking
//!
//! An unbounded wait inside a frame — a `flush`, or a `present` over a
//! region still in flight — completes instantly in simulated time, which
//! is a visible jump. A shell that would rather not see one keeps such
//! damage for the next frame; `examples/demo.rs` shows the pattern. This
//! crate does not enforce it, for the same reason the simulator records
//! faults rather than refusing writes: the point is to see what a shell
//! does.
//!
//! # Input
//!
//! [`Input`] is the viewer's own small type — pointer position in panel
//! coordinates, whether the button is held, and the events since the last
//! frame — so nothing from the windowing library reaches a shell's API.
//! On a device the same shell maps evdev to whatever it does with these,
//! and that mapping is the only thing that differs.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mezzotint::{PanelDriver, PanelRect, RefreshPolicy, SimConfig, SimulatedPanel, UpdateClass};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::window::{Window, WindowId};

/// How the panel and the window are set up.
#[derive(Debug, Clone)]
pub struct Config {
    /// Panel width in pixels.
    pub width: u32,
    /// Panel height in pixels.
    pub height: u32,
    /// Timing and alignment for the simulated controller.
    pub sim: SimConfig,
    /// The driver's flash cadence.
    pub policy: RefreshPolicy,
    /// Window size is the panel size divided by this.
    pub scale: u32,
    /// Outline in-flight regions and invert a flash for its first beat.
    pub overlays: bool,
    /// Print each fault to stderr as it happens, as well as counting it.
    pub log_faults: bool,
    /// The window title; the viewer appends its counters.
    pub title: String,
    /// Which windowing system to use, where there is a choice.
    pub backend: Backend,
}

/// The windowing system, on platforms that offer more than one.
///
/// Only Linux and the BSDs do. The default lets the windowing library
/// pick, which is Wayland when there is a Wayland compositor. Forcing X11
/// is for automation: the X test extension can drive a window with
/// synthetic pointer and key events, and nothing equivalent exists for
/// Wayland. `MEZZOTINT_VIEWER_BACKEND=x11` in the environment does the
/// same from outside, so a shell need not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// Let the windowing library choose.
    #[default]
    Auto,
    /// X11, or XWayland under a Wayland session.
    X11,
    /// Wayland.
    Wayland,
}

impl Backend {
    /// `MEZZOTINT_VIEWER_BACKEND`, if set to `x11` or `wayland`.
    pub fn from_env() -> Self {
        match std::env::var("MEZZOTINT_VIEWER_BACKEND").as_deref() {
            Ok("x11") => Backend::X11,
            Ok("wayland") => Backend::Wayland,
            _ => Backend::Auto,
        }
    }
}

impl Config {
    /// Bytes an RGBA buffer covering the panel takes.
    pub const fn buffer_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

impl Default for Config {
    /// A six-inch panel's worth of pixels, at half size, flashing every
    /// sixth quality update.
    fn default() -> Self {
        Config {
            width: 1072,
            height: 1448,
            sim: SimConfig::default(),
            policy: RefreshPolicy::new(6),
            scale: 2,
            overlays: true,
            log_faults: true,
            title: "mezzotint".to_string(),
            backend: Backend::from_env(),
        }
    }
}

/// A key a shell might care about. Anything else arrives as [`Key::Char`]
/// or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// Space bar.
    Space,
    /// Return.
    Enter,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Home.
    Home,
    /// End.
    End,
    /// A printable character, as typed.
    Char(char),
}

/// Something that happened since the last frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The pointer moved to this panel position.
    PointerMoved {
        /// Column.
        x: u32,
        /// Row.
        y: u32,
    },
    /// The button went down at this panel position.
    Pressed {
        /// Column.
        x: u32,
        /// Row.
        y: u32,
    },
    /// The button came up at this panel position.
    Released {
        /// Column.
        x: u32,
        /// Row.
        y: u32,
    },
    /// A key went down. Repeats are delivered.
    Key(Key),
}

/// The input state at a frame, and what changed since the last one.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Where the pointer is, in panel coordinates, if it is over the
    /// window.
    pub pointer: Option<(u32, u32)>,
    /// Whether the primary button is held.
    pub held: bool,
    /// Everything since the last frame, in order. Cleared after each.
    pub events: Vec<Event>,
}

/// What a shell wants after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Draw and come back next frame.
    Continue,
    /// Close the window and return from [`run`].
    Exit,
}

/// A shell the viewer drives.
pub trait App {
    /// One frame. The panel's clock has advanced and finished updates
    /// have been retired. Present, settle and render here; the glass is
    /// drawn afterwards.
    fn frame(&mut self, driver: &mut PanelDriver<SimulatedPanel>, input: &Input) -> Control;

    /// Text for the title bar, ahead of the viewer's own counters.
    fn status(&self) -> Option<String> {
        None
    }
}

/// Anything that stops the window from opening or running.
pub type Error = Box<dyn std::error::Error>;

/// Open the window and run `app` until it asks to exit or the window is
/// closed.
pub fn run(config: Config, app: impl App) -> Result<(), Error> {
    let mut builder = EventLoop::builder();
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "ios"))
    ))]
    match config.backend {
        Backend::Auto => {}
        Backend::X11 => {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_x11();
        }
        Backend::Wayland => {
            use winit::platform::wayland::EventLoopBuilderExtWayland;
            builder.with_wayland();
        }
    }
    let event_loop = builder.build()?;
    // Poll: the panel's clock is wall time, so there is always a next
    // frame to draw.
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut host = Host::new(config, app);
    event_loop.run_app(&mut host)?;
    match host.failure.take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// The fraction of a flash's duration painted inverted, so it has a beat.
const FLASH_BEAT: f32 = 0.4;

struct Host<A: App> {
    config: Config,
    app: A,
    driver: PanelDriver<SimulatedPanel>,
    input: Input,
    last: Instant,
    faults_seen: usize,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    failure: Option<Error>,
}

impl<A: App> Host<A> {
    fn new(config: Config, app: A) -> Self {
        let panel = SimulatedPanel::with_config(config.width, config.height, config.sim);
        let driver = PanelDriver::with_policy(panel, config.policy);
        Host {
            config,
            app,
            driver,
            input: Input::default(),
            last: Instant::now(),
            faults_seen: 0,
            window: None,
            surface: None,
            failure: None,
        }
    }

    /// Let wall time pass on the panel, retire what finished, run the
    /// shell's frame, and report new faults.
    fn frame(&mut self) -> Control {
        let now = Instant::now();
        let elapsed = now - self.last;
        self.last = now;
        self.driver.panel_mut().advance(elapsed);
        // A zero-deadline flush is a poll: it retires every update that
        // has finished and returns at the first that has not.
        let _ = self.driver.flush_within(Duration::ZERO);

        let control = self.app.frame(&mut self.driver, &self.input);
        self.input.events.clear();

        let faults = &self.driver.panel().faults;
        if faults.len() > self.faults_seen {
            if self.config.log_faults {
                for f in &faults[self.faults_seen..] {
                    eprintln!(
                        "mezzotint-viewer: FAULT: wrote {:?} under live {:?} ({:?})",
                        f.written, f.live, f.token
                    );
                }
            }
            self.faults_seen = faults.len();
        }
        control
    }

    fn title(&self) -> String {
        let p = self.driver.panel();
        let status = self.app.status().map(|s| format!("{s}  "));
        format!(
            "{} — {}t={:.2}s  live={}  updates={}  faults={}  flash in {}",
            self.config.title,
            status.unwrap_or_default(),
            p.now().as_secs_f32(),
            p.in_flight().len(),
            p.updates.len(),
            p.faults.len(),
            self.driver
                .policy()
                .flash_every()
                .saturating_sub(self.driver.policy().since_flash()),
        )
    }

    /// The cursor's panel position for a window position.
    fn to_panel(&self, x: f64, y: f64) -> (u32, u32) {
        let Some(window) = &self.window else {
            return (0, 0);
        };
        let size = window.inner_size();
        let bw = size.width.max(1) as f64;
        let bh = size.height.max(1) as f64;
        let px = ((x.max(0.0) / bw) * self.config.width as f64) as u32;
        let py = ((y.max(0.0) / bh) * self.config.height as f64) as u32;
        (
            px.min(self.config.width.saturating_sub(1)),
            py.min(self.config.height.saturating_sub(1)),
        )
    }

    /// Paint the glass and the overlays into a window buffer of
    /// `bw`×`bh` 0RGB pixels.
    fn draw(&self, buf: &mut [u32], bw: u32, bh: u32) {
        let (w, h) = (self.config.width, self.config.height);
        let p = self.driver.panel();
        let glass = p.glass();
        let now = p.now();
        let beats: Vec<PanelRect> = if self.config.overlays {
            p.in_flight()
                .iter()
                .filter_map(|live| {
                    let sub = p.updates.iter().rev().find(|u| u.token == live.token)?;
                    if sub.class != UpdateClass::Flash {
                        return None;
                    }
                    let dur = sub.completes_at - sub.submitted_at;
                    let beat = Duration::from_secs_f32(dur.as_secs_f32() * FLASH_BEAT);
                    (now < sub.submitted_at + beat).then_some(live.rect)
                })
                .collect()
        } else {
            Vec::new()
        };

        for wy in 0..bh {
            let gy = (u64::from(wy) * u64::from(h) / u64::from(bh)) as u32;
            for wx in 0..bw {
                let gx = (u64::from(wx) * u64::from(w) / u64::from(bw)) as u32;
                let mut v = glass[((gy * w + gx) * 4) as usize];
                if beats
                    .iter()
                    .any(|r| gx >= r.x && gx < r.max_x() && gy >= r.y && gy < r.max_y())
                {
                    v = 255 - v;
                }
                buf[(wy * bw + wx) as usize] = u32::from(v) * 0x010101;
            }
        }
        if !self.config.overlays {
            return;
        }
        // Outline what the controller is still reading, in blue.
        for live in p.in_flight() {
            let r = live.rect;
            let x0 = r.x * bw / w;
            let x1 = ((r.max_x() * bw / w).max(x0 + 1)).min(bw) - 1;
            let y0 = r.y * bh / h;
            let y1 = ((r.max_y() * bh / h).max(y0 + 1)).min(bh) - 1;
            for x in x0..=x1 {
                buf[(y0 * bw + x) as usize] = 0x3060FF;
                buf[(y1 * bw + x) as usize] = 0x3060FF;
            }
            for y in y0..=y1 {
                buf[(y * bw + x0) as usize] = 0x3060FF;
                buf[(y * bw + x1) as usize] = 0x3060FF;
            }
        }
    }
}

fn key_of(key: &WinitKey) -> Option<Key> {
    Some(match key {
        WinitKey::Named(named) => match named {
            NamedKey::Space => Key::Space,
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            _ => return None,
        },
        WinitKey::Character(s) => {
            let mut chars = s.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            Key::Char(c)
        }
        _ => return None,
    })
}

impl<A: App> ApplicationHandler for Host<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let scale = self.config.scale.max(1);
        let attrs = Window::default_attributes()
            .with_title(self.title())
            .with_inner_size(LogicalSize::new(
                (self.config.width / scale).max(1),
                (self.config.height / scale).max(1),
            ))
            .with_resizable(true);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(err) => {
                self.failure = Some(err.into());
                event_loop.exit();
                return;
            }
        };
        let surface = softbuffer::Context::new(window.clone())
            .and_then(|context| softbuffer::Surface::new(&context, window.clone()));
        match surface {
            Ok(surface) => {
                self.window = Some(window);
                self.surface = Some(surface);
            }
            Err(err) => {
                self.failure = Some(err.into());
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let Some(key) = key_of(&event.logical_key) {
                    self.input.events.push(Event::Key(key));
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = self.to_panel(position.x, position.y);
                self.input.pointer = Some((x, y));
                self.input.events.push(Event::PointerMoved { x, y });
            }
            WindowEvent::CursorLeft { .. } => self.input.pointer = None,
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y) = self.input.pointer.unwrap_or((0, 0));
                match state {
                    ElementState::Pressed => {
                        self.input.held = true;
                        self.input.events.push(Event::Pressed { x, y });
                    }
                    ElementState::Released => {
                        self.input.held = false;
                        self.input.events.push(Event::Released { x, y });
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if self.frame() == Control::Exit {
                    event_loop.exit();
                    return;
                }
                // The surface comes out of `self` for the draw, because
                // painting reads the rest of the host while the surface is
                // borrowed mutably.
                let (Some(window), Some(mut surface)) = (self.window.clone(), self.surface.take())
                else {
                    return;
                };
                let size = window.inner_size();
                let painted = match (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
                    (Some(bw), Some(bh)) => surface.resize(bw, bh).and_then(|()| {
                        let mut buf = surface.buffer_mut()?;
                        self.draw(&mut buf, bw.get(), bh.get());
                        buf.present()
                    }),
                    // Minimised: nothing to paint, and nothing wrong.
                    _ => Ok(()),
                };
                self.surface = Some(surface);
                if let Err(err) = painted {
                    self.failure = Some(err.into());
                    event_loop.exit();
                    return;
                }
                window.set_title(&self.title());
            }
            _ => {}
        }
    }
}
