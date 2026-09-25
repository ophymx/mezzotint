//! A stand-in shell in the viewer: a page of bars and a gradient, a page
//! turn on Space, a selection that follows a drag.
//!
//!     cargo run --release --manifest-path viewer/Cargo.toml --example demo
//!
//! mezzotint has no renderer, so the "page" here is synthetic. What the
//! demo is for is the driving: it presents, settles and defers exactly as
//! a real shell would, and the one thing it does that matters for a UI
//! thread is that it never blocks on the panel — damage over a region
//! still in flight waits in `pending` for the next frame.
//!
//! Keys: Space or → turns a page, ← turns back, F flashes, Q or Escape
//! quits. Drag with the mouse to run a selection under the pointer; that
//! is the monochrome update, and releasing is what tells the driver to
//! repaint what it degraded. Watch the fault count in the title stay at
//! zero.

use mezzotint::{PanelDriver, PanelRect, SimulatedPanel, Source, Update, UpdateClass};
use mezzotint_viewer::{App, Config, Control, Event, Input, Key};

const W: u32 = 1072;
const H: u32 = 1448;

/// Where the "photograph" sits on every page. Named as the dither region
/// on quality updates, which is the contract's whole point about where
/// error diffusion belongs.
const PHOTO: PanelRect = PanelRect::new(96, 880, W - 192, 440);

/// Height of the selection band a drag paints.
const BAND: u32 = 28;

struct Shell {
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
    /// The first frame presents the first page.
    first: bool,
}

impl Shell {
    fn new() -> Self {
        let mut shell = Shell {
            page: 1,
            base: vec![0; (W * H * 4) as usize],
            frame: vec![0; (W * H * 4) as usize],
            band: None,
            pending: None,
            first: true,
        };
        shell.render_page();
        shell
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

    fn present(
        &mut self,
        driver: &mut PanelDriver<SimulatedPanel>,
        rect: PanelRect,
        class: UpdateClass,
    ) {
        // Never block the UI thread: if the controller is still reading
        // this region, come back next frame.
        let busy = driver
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
        if let Err(err) = driver.present(src, Some(rect), update) {
            eprintln!("present: {err}");
        }
    }

    fn turn(&mut self, driver: &mut PanelDriver<SimulatedPanel>, forward: bool) {
        self.page = if forward {
            self.page + 1
        } else {
            self.page.saturating_sub(1).max(1)
        };
        self.render_page();
        self.pending = None;
        self.present(driver, PanelRect::full(W, H), UpdateClass::Quality);
    }

    fn drag_to(&mut self, driver: &mut PanelDriver<SimulatedPanel>, y: u32) {
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
        self.present(driver, damage, UpdateClass::Monochrome);
    }

    fn release(&mut self, driver: &mut PanelDriver<SimulatedPanel>) {
        if let Some(old) = self.band.take() {
            self.paint(old, false);
        }
        // The finger lifted. Only the shell knows that, and this is the
        // call that says so; the driver repaints whatever the drag
        // degraded, at full fidelity. The dither region goes along
        // exactly as it does on a present: the repaint can be escalated
        // to a flash of the whole panel when the ghost debt is due, and a
        // flash that forgets where the photograph is posterizes it.
        let src = Source::panel(&self.frame, W, H).expect("frame is a full page");
        if let Err(err) = driver.settle(src, &[PHOTO]) {
            eprintln!("settle: {err}");
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
}

impl App for Shell {
    fn frame(&mut self, driver: &mut PanelDriver<SimulatedPanel>, input: &Input) -> Control {
        if self.first {
            self.first = false;
            self.present(driver, PanelRect::full(W, H), UpdateClass::Quality);
        }
        for event in &input.events {
            match *event {
                Event::Key(Key::Space | Key::Right | Key::PageDown) => self.turn(driver, true),
                Event::Key(Key::Left | Key::PageUp) => self.turn(driver, false),
                Event::Key(Key::Char('f' | 'F')) => {
                    self.pending = None;
                    self.present(driver, PanelRect::full(W, H), UpdateClass::Flash);
                }
                Event::Key(Key::Char('q' | 'Q') | Key::Escape) => return Control::Exit,
                Event::Pressed { y, .. } => self.drag_to(driver, y),
                Event::PointerMoved { y, .. } if input.held => self.drag_to(driver, y),
                Event::Released { .. } => self.release(driver),
                _ => {}
            }
        }
        if let Some((rect, class)) = self.pending.take() {
            self.present(driver, rect, class);
        }
        Control::Continue
    }

    fn status(&self) -> Option<String> {
        Some(format!("page {}", self.page))
    }
}

fn main() {
    let config = Config {
        width: W,
        height: H,
        ..Config::default()
    };
    if let Err(err) = mezzotint_viewer::run(config, Shell::new()) {
        eprintln!("viewer: {err}");
        std::process::exit(1);
    }
}
