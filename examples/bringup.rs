//! Bring up a panel: say what it is, check the pixel packing against the
//! kernel's own numbers, and — with `--drive` — put a test card through
//! every update class and time it on the glass.
//!
//!     cargo run --example bringup --features fbdev  -- [/dev/fb0] [--drive]
//!     cargo run --example bringup --features hwtcon -- [/dev/fb0] [--drive] [--quality gl16|glr16]
//!
//! One tool for every backend, so that a new controller is brought up by
//! adding a backend and nothing else. The stages run in the order a
//! person needs them, and each is meaningless if the one before it lied:
//!
//! 1. **Identify.** The framebuffer's geometry and driver, then whatever
//!    the backend behind that driver can add. If the geometry is wrong
//!    the `#[repr(C)]` structs do not match the kernel's and nothing below
//!    can be trusted. `--example probe` is this stage alone, and writes
//!    nothing.
//! 2. **Self-test.** Seven known colours written to the bottom row and
//!    read back, checked against the channel placement the device itself
//!    reports. Unit tests can only assert the bitfield constants someone
//!    typed while guessing at hardware; this asserts against the panel
//!    present. The bottom row rather than the top, because a framebuffer
//!    console draws top-left and a redraw between the write and the
//!    read-back fails this for a reason that has nothing to do with the
//!    packing. Exits non-zero on a mismatch, so `scripts/fbdev-vm.sh` can
//!    be a gate rather than something a human reads.
//! 3. **Drive**, on `--drive`. A test card goes into a 400x400 region in
//!    the middle of the screen — a sixteen-step grey ramp, colour bars
//!    for a colour filter array, and a one-pixel checker, the worst case
//!    for any waveform — and every class is submitted through
//!    [`PanelDriver`], so the rules the driver enforces are the ones
//!    exercised, with the time from `present` to glass printed for each.
//!    Then the sequence a shell performs: a caret swept across the card
//!    in monochrome strips that each leave their region provisional, a
//!    settle at full fidelity with the ramp declared a dither region, and
//!    a flush before letting go. The region is put back afterwards.
//!
//! The backend is chosen by what the kernel calls its driver. With the
//! `hwtcon` feature on, a `hwtcon` framebuffer is driven through
//! [`HwtconPanel`]; anything else is driven as a bare framebuffer, where
//! nothing is ever in flight, the timings are the cost of the blit, and
//! the stages still run. A controller nobody has transcribed is therefore
//! not refused: it is driven as a plain framebuffer, which is what a shell
//! without a backend for it would do, and the output says so.
//!
//! Run it as root, on the device, while holding the device: the numbers
//! it prints are the ones a backend's documentation quotes, and the
//! picture it draws is the only test of whether a waveform does what its
//! name says.

use std::thread::sleep;
use std::time::{Duration, Instant};

use mezzotint::fbdev::FbdevPanel;
#[cfg(feature = "hwtcon")]
use mezzotint::hwtcon::{HwtconPanel, Waveforms};
#[cfg(feature = "hwtcon")]
use mezzotint::Error;
use mezzotint::{encode, Panel, PanelDriver, PanelRect, Source, Update, UpdateClass};

/// Side of the card the drive stage draws, or the panel's shorter side
/// when that is smaller.
const CARD: u32 = 400;

struct Options {
    device: String,
    drive: bool,
    #[cfg(feature = "hwtcon")]
    quality: Option<u32>,
}

fn usage(message: &str) -> ! {
    eprintln!("bringup: {message}");
    std::process::exit(2)
}

fn options() -> Options {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let device = args
        .iter()
        .find(|a| a.starts_with('/'))
        .cloned()
        .unwrap_or_else(|| "/dev/fb0".to_string());
    let drive = args.iter().any(|a| a == "--drive");
    let quality = args
        .iter()
        .position(|a| a == "--quality")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    #[cfg(feature = "hwtcon")]
    let quality = match quality {
        None => None,
        Some("gl16") => Some(mezzotint::hwtcon::uapi::WAVEFORM_MODE_GL16),
        Some("glr16") => Some(mezzotint::hwtcon::uapi::WAVEFORM_MODE_GLR16),
        Some(other) => usage(&format!("--quality takes gl16 or glr16, not {other}")),
    };
    #[cfg(not(feature = "hwtcon"))]
    if quality.is_some() {
        usage("--quality picks a hwtcon waveform, and this build has no hwtcon");
    }
    Options {
        device,
        drive,
        #[cfg(feature = "hwtcon")]
        quality,
    }
}

fn main() {
    let opts = options();
    let device = &opts.device;

    // Stages 1 and 2 are the framebuffer's, whichever controller sits
    // behind it: the packing is fbdev's concern, and the backend that
    // takes over below opens the same device again on its own terms.
    let mut fb = match FbdevPanel::open_path(device) {
        Ok(fb) => fb,
        Err(err) => {
            println!("{device}: {err}");
            std::process::exit(1);
        }
    };
    println!("{device}: driver {:?}, {}", fb.driver_id(), fb.describe());
    let info = fb.info();
    println!(
        "  asks for {}x{} {:?}",
        info.width, info.height, info.format
    );

    let failures = selftest(&mut fb);
    if failures == 0 {
        println!("self-test PASS");
    } else {
        println!("self-test FAIL: {failures} check(s)");
        std::process::exit(1);
    }

    let side = CARD.min(info.width).min(info.height);
    let region = PanelRect::new(
        (info.width - side) / 2,
        (info.height - side) / 2,
        side,
        side,
    );
    // What is under the card now, as RGBA, so the device can be left as
    // it was found. Read here, through the bare framebuffer's channel
    // placement, because only it knows how to spell a pixel back out.
    let saved = snapshot(&fb, region);
    drop(fb);

    #[cfg(feature = "hwtcon")]
    match HwtconPanel::open_path(device) {
        Ok(panel) => {
            let mut waveforms = Waveforms::default();
            if let Some(quality) = opts.quality {
                waveforms.quality = quality;
            }
            let panel = panel.with_waveforms(waveforms);
            println!("{device}: {}", panel.describe());
            match panel.panel_info() {
                Ok(info) => println!("  {info:?}"),
                Err(err) => println!("  panel info: {err}"),
            }
            match panel.temperature() {
                Ok(t) => println!("  {t} C"),
                Err(err) => println!("  temperature: {err}"),
            }
            if opts.drive {
                drive(panel, region, saved.as_deref());
            }
            return;
        }
        Err(Error::UnexpectedDriver { .. }) => {
            println!("{device}: no backend for this driver, driving it as a bare framebuffer");
        }
        Err(err) => {
            println!("{device}: {err}");
            std::process::exit(1);
        }
    }
    #[cfg(not(feature = "hwtcon"))]
    println!("{device}: built without a controller backend, driving it as a bare framebuffer");

    if !opts.drive {
        return;
    }
    match FbdevPanel::open_path(device) {
        Ok(panel) => drive(panel, region, saved.as_deref()),
        Err(err) => {
            println!("{device}: {err}");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------
// Stage 2: the packing, against the device's own numbers
// ---------------------------------------------------------------------

/// Pack a colour the way the device says it packs them.
///
/// `channels` is `(offset, length)` per channel from the device's own
/// bitfield report, so the expectation comes from the kernel's placement
/// rather than from anything this crate believes.
fn expected(r: u8, g: u8, b: u8, channels: [(u32, u32); 3]) -> u32 {
    let mut word = 0u32;
    for (value, (offset, length)) in [r, g, b].into_iter().zip(channels) {
        let fitted = match length {
            0 => 0,
            1..=8 => u32::from(value) >> (8 - length),
            _ => u32::from(value) << (length - 8),
        };
        word |= fitted << offset;
    }
    word
}

/// Unpack one channel back to eight bits.
fn widen(value: u32, length: u32) -> u8 {
    match length {
        0 => 0,
        1..=8 => (value << (8 - length)) as u8,
        _ => (value >> (length - 8)) as u8,
    }
}

/// Write known colours, read them back, count the mismatches.
fn selftest(panel: &mut FbdevPanel) -> usize {
    let cases: [(&str, [u8; 3]); 7] = [
        ("black", [0x00, 0x00, 0x00]),
        ("white", [0xFF, 0xFF, 0xFF]),
        ("red", [0xFF, 0x00, 0x00]),
        ("green", [0x00, 0xFF, 0x00]),
        ("blue", [0x00, 0x00, 0xFF]),
        ("mid grey", [0x80, 0x80, 0x80]),
        ("odd mix", [0x12, 0x34, 0x56]),
    ];

    let row = panel.info().height - 1;
    // One case per pixel along the bottom row, handed over as a strip
    // rather than a whole page: a bring-up tool on a small device should
    // not need a panel-sized allocation to write seven pixels.
    let strip_rect = PanelRect::new(0, row, cases.len() as u32, 1);
    // What is there now, to put back afterwards: on e-ink these pixels
    // would otherwise surface at the next full refresh.
    let before = snapshot(panel, strip_rect);
    let mut strip = vec![0u8; cases.len() * 4];
    for (i, (_, [r, g, b])) in cases.iter().enumerate() {
        strip[i * 4..i * 4 + 4].copy_from_slice(&[*r, *g, *b, 0xFF]);
    }
    if let Err(err) = Source::at(&strip, strip_rect).and_then(|src| panel.blit(src, strip_rect)) {
        println!("  self-test blit: {err}");
        return 1;
    }

    let channels = panel.channels();
    println!("  channels: {channels:?}");
    let mut failures = 0;
    for (i, (name, [r, g, b])) in cases.iter().enumerate() {
        let got = panel.read_pixel(i as u32, row).expect("pixel is on screen");
        match channels {
            Some(channels) => {
                let want = expected(*r, *g, *b, channels);
                let ok = got == want;
                failures += usize::from(!ok);
                println!(
                    "  {name:>8}  wrote {r:02x}{g:02x}{b:02x}  read {got:#010x}  \
                     want {want:#010x}  {}",
                    if ok { "ok" } else { "MISMATCH" }
                );
            }
            None => {
                // Greyscale or mono: no channel placement to check
                // against, so only the ordering property is meaningful.
                // On a 1bpp panel `read_pixel` returns the bit itself,
                // so the interesting reading is 0 versus 1.
                println!(
                    "  {name:>8}  wrote {r:02x}{g:02x}{b:02x}  read {got:#010x}  \
                     (no channel placement)"
                );
            }
        }
    }

    if channels.is_some() {
        // Distinct colours must not collapse onto one word: a stride or
        // byte-order error often shows up as everything reading alike.
        let mut seen: Vec<u32> = (0..cases.len())
            .filter_map(|i| panel.read_pixel(i as u32, row))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        if seen.len() < cases.len() {
            println!(
                "  DISTINCTNESS: {} of {} words differ",
                seen.len(),
                cases.len()
            );
            failures += 1;
        }
    }

    match before {
        Some(before) => {
            if let Err(err) =
                Source::at(&before, strip_rect).and_then(|src| panel.blit(src, strip_rect))
            {
                println!("  bottom row not put back: {err}");
            }
        }
        None => println!("  bottom row not put back: no channel placement to read it through"),
    }
    failures
}

/// The pixels under `region` as tightly packed RGBA, or `None` on a
/// greyscale or mono framebuffer, where a word has no channels to
/// unpack and the region is not put back.
fn snapshot(panel: &FbdevPanel, region: PanelRect) -> Option<Vec<u8>> {
    let channels = panel.channels()?;
    let mut rgba = Vec::with_capacity(region.area() as usize * 4);
    for y in region.y..region.max_y() {
        for x in region.x..region.max_x() {
            let word = panel.read_pixel(x, y)?;
            for (offset, length) in channels {
                let mask = if length >= 32 {
                    u32::MAX
                } else {
                    (1u32 << length) - 1
                };
                rgba.push(widen((word >> offset) & mask, length));
            }
            rgba.push(0xFF);
        }
    }
    Some(rgba)
}

// ---------------------------------------------------------------------
// Stage 3: the card, every class, and the shell's sequence
// ---------------------------------------------------------------------

/// A sixteen-step ramp over the top half, colour bars over the next
/// quarter, and a one-pixel checker over the last.
fn card(side: u32) -> Vec<u8> {
    let mut card = vec![0u8; (side * side * 4) as usize];
    for y in 0..side {
        for x in 0..side {
            let (r, g, b) = if y < side / 2 {
                let v = ((x * 16 / side) * 17) as u8;
                (v, v, v)
            } else if y < side * 3 / 4 {
                match x * 6 / side {
                    0 => (255, 0, 0),
                    1 => (255, 255, 0),
                    2 => (0, 255, 0),
                    3 => (0, 255, 255),
                    4 => (0, 0, 255),
                    _ => (255, 0, 255),
                }
            } else if (x + y) % 2 == 0 {
                (0, 0, 0)
            } else {
                (255, 255, 255)
            };
            let at = ((y * side + x) * 4) as usize;
            card[at..at + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    card
}

fn fill(page: &mut [u8], stride: u32, rect: PanelRect, value: u8) {
    for y in rect.y..rect.max_y() {
        for x in rect.x..rect.max_x() {
            let at = ((y * stride + x) * 4) as usize;
            page[at..at + 4].copy_from_slice(&[value, value, value, 0xFF]);
        }
    }
}

fn drive<P: Panel>(panel: P, region: PanelRect, saved: Option<&[u8]>) {
    let mut driver = PanelDriver::new(panel);
    let info = driver.info();
    let side = region.w;

    // Where a photograph would be, if this were a page with one on it:
    // the ramp. Only a consumer can know this, which is why it is a
    // parameter of `present` and not a flag on the panel.
    let ramp_in_card = PanelRect::new(0, 0, side, side / 2);
    let ramp = PanelRect::new(region.x, region.y, side, side / 2);
    let within = [ramp];

    // The consumer's half of the blit convention: reduce to the format
    // the panel asked for. A no-op on an RGBA panel, where the controller
    // does the reduction itself — which is the point of the convention.
    let mut card = card(side);
    encode::quantize_for(
        &mut card,
        side,
        side,
        PanelRect::full(side, side),
        info.format,
        &[ramp_in_card],
    );
    let inverted: Vec<u8> = card
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [255 - p[0], 255 - p[1], 255 - p[2], 255])
        .collect();

    let classes = [
        (UpdateClass::Quality, "Quality", &card),
        (UpdateClass::Fast, "Fast", &inverted),
        (UpdateClass::Monochrome, "Monochrome", &card),
        (UpdateClass::Flash, "Flash", &inverted),
    ];
    for (class, name, pixels) in classes {
        let src = Source::at(pixels, region).expect("card covers its region");
        let update = Update::new(class).dithering_within(&within);
        let t0 = Instant::now();
        let token = if class == UpdateClass::Flash {
            // A flash the consumer asks for repaints the whole panel, and
            // `PanelDriver` rightly refuses one backed by a 400x400 strip.
            // Going to the panel directly here is deliberate: the point is
            // to time the waveform, not to flash whatever else is on the
            // glass.
            let panel = driver.panel_mut();
            panel
                .blit(src, region)
                .and_then(|()| panel.submit(region, update))
                .map(Some)
        } else {
            driver.present(src, Some(region), update)
        };
        let token = match token {
            Ok(Some(token)) => token,
            Ok(None) => {
                println!("{name}: nothing to do");
                continue;
            }
            Err(err) => {
                println!("{name}: present: {err}");
                continue;
            }
        };
        let submitted = t0.elapsed();
        match driver.panel_mut().wait(token, None) {
            Ok(_) => println!(
                "{name:<10} submitted {:>4} ms, on glass {:>4} ms",
                submitted.as_millis(),
                t0.elapsed().as_millis()
            ),
            Err(err) => println!("{name}: wait: {err}"),
        }
        sleep(Duration::from_millis(1500));
    }

    // The sequence a shell performs, in miniature. A caret sweeping
    // across the checker band: one monochrome update per step, each
    // leaving its region provisional. The driver is accruing the debt.
    let mut page = card.clone();
    let band_y = side * 3 / 4;
    let (step_w, band_h) = (side / 8, side / 4);
    for step in 0..8 {
        let in_card = PanelRect::new(step * step_w, band_y, step_w, band_h);
        fill(
            &mut page,
            side,
            in_card,
            if step % 2 == 0 { 0x00 } else { 0xFF },
        );
        let caret = PanelRect::new(region.x + in_card.x, region.y + band_y, step_w, band_h);
        let src = Source::at(&page, region).expect("page covers its region");
        if let Err(err) = driver.present(src, Some(caret), UpdateClass::Monochrome) {
            println!("caret {step}: {err}");
        }
        sleep(Duration::from_millis(120));
    }
    println!("owed a clean pass over {:?}", driver.provisional());

    // The gesture ended. Only a shell knows that, so only a shell can say
    // so — and the repaint goes at full fidelity over everything the
    // gesture touched, not just its last step.
    sleep(Duration::from_millis(500));
    let t0 = Instant::now();
    let src = Source::at(&page, region).expect("page covers its region");
    match driver.settle(src, &within) {
        Ok(Some(token)) => match driver.panel_mut().wait(token, None) {
            Ok(_) => println!("settled, on glass {:>4} ms", t0.elapsed().as_millis()),
            Err(err) => println!("settle: wait: {err}"),
        },
        Ok(None) => println!("settle: nothing owed"),
        Err(err) => println!("settle: {err}"),
    }
    sleep(Duration::from_millis(1500));

    // Leave the glass as it was found.
    match saved {
        Some(saved) => {
            let src = Source::at(saved, region).expect("snapshot covers its region");
            match driver.present(src, Some(region), UpdateClass::Quality) {
                Ok(Some(token)) => {
                    if let Err(err) = driver.panel_mut().wait(token, None) {
                        println!("restore: wait: {err}");
                    }
                }
                Ok(None) => {}
                Err(err) => println!("restore: {err}"),
            }
        }
        None => println!("not restored: no channel placement to read the old pixels back through"),
    }

    // A shell owes this before it tears the panel down, and the habit
    // should be visible here.
    if let Err(err) = driver.flush() {
        println!("flush: {err}");
    }
}
