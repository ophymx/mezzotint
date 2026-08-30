//! What a reduction pass costs, against what a refresh costs.
//!
//!     cargo bench
//!
//! `encode::quantize` runs inside `submit` on a device, so its cost is
//! spent against a panel refresh of roughly 450ms — which is what makes a
//! few milliseconds the right answer and seventy the wrong one. The
//! numbers in `encode`'s own documentation come from here, and the point
//! of keeping it is that the interesting case is not the cheap one: a
//! pass with no diffusion regions is fast however it is written, and the
//! per-pixel version of this was ten times slower once regions existed.
//!
//! Not a gate — timings on a build machine are not timings on an A9 — but
//! a claim in a doc comment should have something behind it that can be
//! re-run.

use std::time::Instant;

use mezzotint::{encode, PanelRect};

/// A Kobo Clara/Libra-class panel, which is the size the doc comment
/// quotes.
const W: u32 = 1448;
const H: u32 = 1072;

fn main() {
    let mut page = vec![0u8; (W * H * 4) as usize];
    for (i, px) in page.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        // Something with structure in it, so the diffusion has real error
        // to carry rather than a flat field it can shortcut.
        let v = ((i % 251) ^ (i / W as usize)) as u8;
        *px = [v, v, v, 0xFF];
    }
    let full = PanelRect::full(W, H);

    println!("{W}x{H}, 16 levels");
    for zones in [0usize, 1, 4, 8] {
        let regions: Vec<PanelRect> = (0..zones)
            .map(|i| PanelRect::new(40, 40 + (i as u32) * 120, 600, 90))
            .collect();
        let mut buf = page.clone();
        encode::quantize(&mut buf, W, H, full, 16, &regions);

        let runs = 10;
        let mut total = std::time::Duration::ZERO;
        for _ in 0..runs {
            let mut buf = page.clone();
            let start = Instant::now();
            encode::quantize(&mut buf, W, H, full, 16, &regions);
            total += start.elapsed();
        }
        println!("  {zones} dither regions: {:?}", total / runs);
    }
}
