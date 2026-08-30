//! Drive a framebuffer through the whole seam, with a test pattern
//! instead of a document.
//!
//!     cargo run --example pattern -- [/dev/fb0]
//!
//! Deliberately written the way a device shell would be, rather than the
//! short way. It builds a page, states damage, goes through
//! [`PanelDriver`] rather than calling the panel directly, marks a region
//! provisional with a fast update and then settles it, and flushes before
//! it lets go — so the collision rules, the ghost cadence and the cleanup
//! pass are all exercised against a screen that shows the result.
//!
//! The pattern is chosen to make reduction visible: a full greyscale ramp
//! across the top, a block of flat mid-grey below it, and a caption band
//! of hard black and white. On a mono framebuffer the ramp and the block
//! are inside a declared dither region and the caption band is not, which
//! is exactly the distinction the contract exists to carry — diffused
//! through a photograph, plain through text.
//!
//! Needs write access to the framebuffer (`video` group, or root) and a
//! console that is not being redrawn underneath it: `chvt` to a free one,
//! or run it in a VM.

use std::thread::sleep;
use std::time::Duration;

use mezzotint::fbdev::FbdevPanel;
use mezzotint::{encode, PanelDriver, PanelRect, Source, UpdateClass};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/fb0".to_string());
    let panel = FbdevPanel::open_path(&device)?;
    // Say what was found: "it drew something" is not the same as "it drew
    // it right", and on an unfamiliar panel this line is the difference.
    eprintln!("pattern: {device} — {}", panel.describe());

    let mut driver = PanelDriver::new(panel);
    let info = driver.info();
    let (w, h) = (info.width, info.height);

    // Where a photograph would be, if this were a page with one on it.
    // Only a consumer can know this, which is the whole reason it is a
    // parameter and not a flag.
    let images = [PanelRect::new(0, 0, w, h * 2 / 3)];

    let mut page = vec![0u8; info.buffer_len()];
    paint(&mut page, w, h);
    encode::quantize_for(&mut page, w, h, info.bounds(), info.format, &images);
    let src = Source::panel(&page, w, h)?;

    // The page itself.
    driver.present(src, None, UpdateClass::Quality)?;
    sleep(Duration::from_secs(1));

    // A caret sweeping across the caption band: one fast update per step,
    // each leaving its region provisional. On e-ink these would be A2
    // waveforms and would look chunky; the driver is accruing the debt.
    let band = h * 5 / 6;
    for step in 0..8 {
        let caret = PanelRect::new(step * (w / 8), band, w / 8, h / 12).clamped(w, h);
        fill(&mut page, w, caret, if step % 2 == 0 { 0x00 } else { 0xFF });
        let src = Source::panel(&page, w, h)?;
        driver.present(src, Some(caret), UpdateClass::Monochrome)?;
        sleep(Duration::from_millis(120));
    }
    eprintln!("pattern: owed a clean pass over {:?}", driver.provisional());

    // The gesture ended. Only a shell knows that, so only a shell can say
    // so — and the repaint goes at full fidelity over everything the
    // gesture touched, not just its last step.
    sleep(Duration::from_millis(500));
    let src = Source::panel(&page, w, h)?;
    driver.settle(src, &images)?;

    // Nothing is ever in flight on a framebuffer, but a shell owes this
    // before it tears the panel down and the habit should be visible here.
    driver.flush()?;
    Ok(())
}

/// A ramp, a flat mid-grey field, and a hard black-and-white band.
fn paint(page: &mut [u8], w: u32, h: u32) {
    for y in 0..h {
        for x in 0..w {
            let at = ((y * w + x) * 4) as usize;
            let v = if y < h / 3 {
                // A full sweep, so a reduction has something to lose.
                (x * 255 / w.max(1)) as u8
            } else if y < h * 2 / 3 {
                // Flat 40% grey: solid black at two levels without
                // diffusion, and recognisably grey with it.
                100
            } else {
                // Hard edges, the way text is: dithering here would only
                // stipple them.
                if (x / 16) % 2 == 0 {
                    0x00
                } else {
                    0xFF
                }
            };
            page[at..at + 4].copy_from_slice(&[v, v, v, 0xFF]);
        }
    }
}

fn fill(page: &mut [u8], w: u32, rect: PanelRect, value: u8) {
    for y in rect.y..rect.max_y() {
        for x in rect.x..rect.max_x() {
            let at = ((y * w + x) * 4) as usize;
            page[at..at + 4].copy_from_slice(&[value, value, value, 0xFF]);
        }
    }
}
