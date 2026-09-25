//! A window onto the simulated panel, so a shell can be *watched* rather
//! than only asserted about.
//!
//!     cargo run --release --manifest-path viewer/Cargo.toml
//!
//! The library's `SimulatedPanel` keeps updates in flight on a clock the
//! caller advances. Here the caller is a frame loop and the clock is wall
//! time, so a page turn lands after its 450ms and a drag strip after its
//! 120ms, and the latency of each class is something you see. What you
//! are looking at is the panel's glass, scaled to the window, with two
//! overlays the simulator makes possible and a device does not: the
//! regions the controller is still busy with, outlined, and a flash
//! painted inverted for its first beat. The title bar carries the fault
//! count, which is the number of times something wrote under a live
//! update — the rule the driver exists to hold, and here it is a number
//! you can watch stay at zero.
//!
//! Keys: Space or → turns a page, ← turns back, F flashes, Q quits.
//! Drag with the mouse to run a selection under the pointer: that is the
//! monochrome update, and releasing is what tells the driver to repaint
//! what it degraded.
//!
//! The shell in here is a stand-in — mezzotint has no renderer, so the
//! "page" is bars and a gradient — but it drives the driver exactly as a
//! real shell would, including the one thing a real shell has to get
//! right on its UI thread: it never blocks on the panel. Damage that
//! overlaps a live update waits in `pending` for the next frame instead.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mezzotint::{
    PanelDriver, PanelRect, RefreshPolicy, SimulatedPanel, Source, Update, UpdateClass,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// A six-inch panel's worth of pixels; shown at half size.
const W: u32 = 1072;
const H: u32 = 1448;
const SCALE: u32 = 2;

/// Where the "photograph" sits on every page. Named as the dither region
/// on quality updates, which is the contract's whole point about where
/// error diffusion belongs.
const PHOTO: PanelRect = PanelRect::new(96, 880, W - 192, 440);

/// Height of the selection band a drag paints.
const BAND: u32 = 28;

/// The fraction of a flash's duration painted inverted, so it has a beat.
const FLASH_BEAT: f32 = 0.4;

// ---------------------------------------------------------------------
// The stand-in shell: a page, a selection, and the driver between them
// and the glass.
// ---------------------------------------------------------------------

struct Shell {
    driver: PanelDriver<SimulatedPanel>,
    page: u32,
    /// The page as rendered, RGBA in panel orientation.
    base: Vec<u8>,
    /// `base` with the current selection band painted over it. What the
    /// driver is handed.
    frame: Vec<u8>,
    /// The band currently on the frame, if any.
    band: Option<PanelRect>,
    /// Damage that could not be presented yet because its region was
    /// still live, and the class it wanted.
    pending: Option<(PanelRect, UpdateClass)>,
    dragging: bool,
    last: Instant,
    faults_seen: usize,
}

impl Shell {
    fn new() -> Self {
        let panel = SimulatedPanel::new(W, H);
        let mut shell = Shell {
            driver: PanelDriver::with_policy(panel, RefreshPolicy::new(6)),
            page: 1,
            base: vec![0; (W * H * 4) as usize],
            frame: vec![0; (W * H * 4) as usize],
            band: None,
            pending: None,
            dragging: false,
            last: Instant::now(),
            faults_seen: 0,
        };
        shell.render_page();
        shell.present(shell.bounds(), UpdateClass::Quality);
        shell
    }

    fn bounds(&self) -> PanelRect {
        PanelRect::full(W, H)
    }

    /// Bars for paragraphs, a gradient with a disc for a photograph. The
    /// bars have soft edges so a monochrome update visibly degrades them.
    fn render_page(&mut self) {
        let seed = self.page.wrapping_mul(2654435761);
        for y in 0..H {
            for x in 0..W {
                let mut v = 255u8;
                let in_photo =
                    x >= PHOTO.x && x < PHOTO.max_x() && y >= PHOTO.y && y < PHOTO.max_y();
                if in_photo {
                    let px = (x - PHOTO.x) as f32 / PHOTO.w as f32;
                    let py = (y - PHOTO.y) as f32 / PHOTO.h as f32;
                    let (cx, cy) = (0.35 + 0.3 * ((seed % 7) as f32 / 7.0), 0.5);
                    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                    let disc = (1.0 - (d / 0.28).min(1.0)).powf(0.6);
                    let grad = 0.25 + 0.6 * px;
                    v = (255.0 * (grad * (1.0 - disc) + 0.08 * disc)) as u8;
                } else if (96..W - 96).contains(&x) && (120..820).contains(&y) {
                    // Lines of "text": 36px pitch, glyph body 18px, each
                    // line a run of words whose lengths come from the seed.
                    let line = (y - 120) / 36;
                    let yy = (y - 120) % 36;
                    let mut h = seed ^ line.wrapping_mul(0x9E3779B9);
                    let mut xx = 96u32;
                    let last_line = line == 19 || ((h >> 5).is_multiple_of(9) && line > 2);
                    let end = if last_line {
                        96 + 300 + (h % 400)
                    } else {
                        W - 96
                    };
                    while xx < end {
                        h ^= h << 13;
                        h ^= h >> 17;
                        h ^= h << 5;
                        let word = 40 + h % 120;
                        let gap = 14;
                        if x >= xx && x < (xx + word).min(end) && (8..26).contains(&yy) {
                            // Soft edge: 2px ramp top and bottom, so the
                            // bar is antialiased like a glyph.
                            let edge = (yy - 8).min(25 - yy);
                            v = match edge {
                                0 => 170,
                                1 => 90,
                                _ => 30,
                            };
                            // A lighter "italic" word now and then.
                            if (h >> 9).is_multiple_of(11) {
                                v = v.saturating_add(90);
                            }
                        }
                        xx += word + gap;
                    }
                }
                let at = ((y * W + x) * 4) as usize;
                self.base[at..at + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        self.frame.copy_from_slice(&self.base);
        self.band = None;
    }

    fn present(&mut self, rect: PanelRect, class: UpdateClass) {
        // Never block the UI thread: if the controller is still reading
        // this region, come back next frame.
        let busy = self
            .driver
            .panel()
            .in_flight()
            .iter()
            .any(|u| u.rect.intersects(rect));
        if busy {
            self.pending = Some(match self.pending.take() {
                Some((r, c)) => (r.union(rect), c.max(class)),
                None => (rect, class),
            });
            return;
        }
        let src = Source::panel(&self.frame, W, H).expect("frame is a full page");
        let update = match class {
            UpdateClass::Quality | UpdateClass::Flash => {
                Update::new(class).dithering_within(&[PHOTO])
            }
            _ => Update::new(class),
        };
        if let Err(err) = self.driver.present(src, Some(rect), update) {
            eprintln!("present: {err}");
        }
    }

    fn turn(&mut self, forward: bool) {
        self.page = if forward {
            self.page + 1
        } else {
            self.page.saturating_sub(1).max(1)
        };
        self.render_page();
        self.pending = None;
        self.present(self.bounds(), UpdateClass::Quality);
    }

    fn flash(&mut self) {
        self.pending = None;
        self.present(self.bounds(), UpdateClass::Flash);
    }

    fn drag_to(&mut self, y: u32) {
        let band = PanelRect::new(0, y.saturating_sub(BAND / 2).min(H - BAND), W, BAND);
        if self.band == Some(band) {
            return;
        }
        let mut damage = band;
        if let Some(old) = self.band.take() {
            self.paint(old, false);
            damage = damage.union(old);
        }
        self.paint(band, true);
        self.band = Some(band);
        self.present(damage, UpdateClass::Monochrome);
    }

    fn release(&mut self) {
        self.dragging = false;
        if let Some(old) = self.band.take() {
            self.paint(old, false);
        }
        // The finger lifted. Only the shell knows that, and this is the
        // call that says so; the driver repaints whatever the drag
        // degraded, at full fidelity.
        let src = Source::panel(&self.frame, W, H).expect("frame is a full page");
        match self.driver.settle(src, &[]) {
            Ok(_) => {}
            Err(err) => eprintln!("settle: {err}"),
        }
    }

    /// Invert `rect` on the frame, or restore it from the page.
    fn paint(&mut self, rect: PanelRect, inverted: bool) {
        for y in rect.y..rect.max_y() {
            let at = ((y * W + rect.x) * 4) as usize;
            let len = (rect.w * 4) as usize;
            let row = &mut self.frame[at..at + len];
            row.copy_from_slice(&self.base[at..at + len]);
            if inverted {
                for px in row.as_chunks_mut::<4>().0 {
                    px[0] = 255 - px[0];
                    px[1] = px[0];
                    px[2] = px[0];
                }
            }
        }
    }

    /// One frame: let wall time pass on the panel, retire what finished,
    /// and try again with anything that had to wait.
    fn tick(&mut self) {
        let now = Instant::now();
        let elapsed = now - self.last;
        self.last = now;
        self.driver.panel_mut().advance(elapsed);
        // A zero-deadline flush is a poll: it retires every update that has
        // finished and returns at the first that has not. The idiom for a
        // UI thread.
        let _ = self.driver.flush_within(Duration::ZERO);
        if let Some((rect, class)) = self.pending.take() {
            self.present(rect, class);
        }
        let faults = self.driver.panel().faults.len();
        if faults > self.faults_seen {
            for f in &self.driver.panel().faults[self.faults_seen..] {
                eprintln!(
                    "FAULT: wrote {:?} under live {:?} ({:?})",
                    f.written, f.live, f.token
                );
            }
            self.faults_seen = faults;
        }
    }

    fn title(&self) -> String {
        let p = self.driver.panel();
        format!(
            "mezzotint — page {}  t={:.2}s  live={}  updates={}  faults={}  flash in {}",
            self.page,
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

    /// Paint the glass and the overlays into a window buffer of `bw`×`bh`
    /// 0RGB pixels.
    fn draw(&self, buf: &mut [u32], bw: u32, bh: u32) {
        let p = self.driver.panel();
        let glass = p.glass();
        // Which regions are mid-flash right now.
        let now = p.now();
        let beats: Vec<PanelRect> = p
            .in_flight()
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
            .collect();

        for wy in 0..bh {
            let gy = (wy as u64 * H as u64 / bh as u64) as u32;
            for wx in 0..bw {
                let gx = (wx as u64 * W as u64 / bw as u64) as u32;
                let mut v = glass[((gy * W + gx) * 4) as usize];
                if beats
                    .iter()
                    .any(|r| gx >= r.x && gx < r.max_x() && gy >= r.y && gy < r.max_y())
                {
                    v = 255 - v;
                }
                buf[(wy * bw + wx) as usize] = u32::from(v) * 0x010101;
            }
        }
        // Outline what the controller is still reading, in blue.
        for live in p.in_flight() {
            let r = live.rect;
            let x0 = r.x * bw / W;
            let x1 = ((r.max_x() * bw / W).max(x0 + 1)).min(bw) - 1;
            let y0 = r.y * bh / H;
            let y1 = ((r.max_y() * bh / H).max(y0 + 1)).min(bh) - 1;
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

// ---------------------------------------------------------------------
// The window.
// ---------------------------------------------------------------------

struct App {
    shell: Shell,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    cursor_y: f64,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.shell.title())
            .with_inner_size(LogicalSize::new(W / SCALE, H / SCALE))
            .with_resizable(true);
        let window = Rc::new(event_loop.create_window(attrs).expect("window"));
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.window = Some(window);
        self.surface = Some(surface);
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
                match event.logical_key {
                    Key::Named(NamedKey::Space | NamedKey::ArrowRight | NamedKey::PageDown) => {
                        self.shell.turn(true)
                    }
                    Key::Named(NamedKey::ArrowLeft | NamedKey::PageUp) => self.shell.turn(false),
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Character(ref c) if c.eq_ignore_ascii_case("f") => self.shell.flash(),
                    Key::Character(ref c) if c.eq_ignore_ascii_case("q") => event_loop.exit(),
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_y = position.y;
                if self.shell.dragging {
                    let y = self.panel_y();
                    self.shell.drag_to(y);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => {
                    self.shell.dragging = true;
                    let y = self.panel_y();
                    self.shell.drag_to(y);
                }
                ElementState::Released => self.shell.release(),
            },
            WindowEvent::RedrawRequested => {
                self.shell.tick();
                let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
                    return;
                };
                let size = window.inner_size();
                let (Some(bw), Some(bh)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                else {
                    return;
                };
                surface.resize(bw, bh).expect("resize");
                let mut buf = surface.buffer_mut().expect("buffer");
                self.shell.draw(&mut buf, bw.get(), bh.get());
                buf.present().expect("present");
                window.set_title(&self.shell.title());
            }
            _ => {}
        }
    }
}

impl App {
    /// The cursor's row in panel coordinates.
    fn panel_y(&self) -> u32 {
        let Some(window) = &self.window else {
            return 0;
        };
        let bh = window.inner_size().height.max(1) as f64;
        ((self.cursor_y.max(0.0) / bh) * H as f64) as u32
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("event loop");
    // Poll: the panel's clock is wall time, so there is always a next
    // frame to draw.
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        shell: Shell::new(),
        window: None,
        surface: None,
        cursor_y: 0.0,
    };
    event_loop.run_app(&mut app).expect("event loop");
}
