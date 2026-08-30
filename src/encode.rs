//! Reducing RGBA to what a panel can actually show.
//!
//! Two things live here and they answer different questions. Packing —
//! turning a pixel into the device's own spelling — belongs to a backend
//! and is in the backend. *Reduction* — how many grey levels, and where
//! the error goes — is arithmetic, the same on every device, and easy to
//! get subtly wrong, so it is here where it can be tested against a `Vec`
//! rather than against a screen.
//!
//! Everything works in place on a panel-orientation RGBA buffer and
//! allocates nothing, so it is reachable without `alloc` and cheap enough
//! to run inside [`submit`](crate::Panel::submit). On a 1448x1072 page a
//! full-buffer pass measures around 7ms with no diffusion regions and
//! around 14ms with eight of them, against a panel refresh of roughly
//! 450ms. `benches/quantize.rs` is where those come from.
//!
//! The second figure is the one worth quoting, because it is the case the
//! feature exists for and the one an implementation gets wrong: testing
//! zone membership per pixel rather than stepping between zone boundaries
//! made it 70ms, and nothing in the cheap case would have shown it.

use crate::{PanelRect, PixelFormat};

/// Rec. 709 luminance.
///
/// One definition, used by the reducer here and by every backend's
/// packing, so that a greyscale framebuffer and a greyscale panel agree
/// about what a colour looks like. Two implementations that differ in the
/// last bit produce two subtly different pages and no error.
///
/// Fixed point rather than floating: `f32::round` is not in `core`, and
/// this has to work on a host with no floating point at all. The three
/// weights are the Rec. 709 coefficients scaled by 2^16 and are chosen to
/// sum to exactly 65536, which is what makes a grey pixel come back
/// unchanged — the property a second pass over an already-reduced buffer
/// depends on.
pub fn luminance(r: u8, g: u8, b: u8) -> u8 {
    const RED: u32 = 13933; // 0.2126
    const GREEN: u32 = 46871; // 0.7152
    const BLUE: u32 = 4732; // 0.0722
    const HALF: u32 = 1 << 15;
    let lum = u32::from(r) * RED + u32::from(g) * GREEN + u32::from(b) * BLUE;
    ((lum + HALF) >> 16) as u8
}

/// The nearest of `levels` evenly spaced grey values to `value`.
///
/// `levels` is clamped up to 2: fewer than two is not a picture. Integer
/// arithmetic throughout, so the round trip is exact — level 15 of 16 is
/// 255 and not 254, which matters because a page whose white is one step
/// short of white looks washed out and nothing reports it.
pub fn nearest_level(value: u8, levels: u8) -> u8 {
    let n = u32::from(levels.max(2)) - 1;
    let index = (u32::from(value) * n + 127) / 255;
    (((index * 255) + n / 2) / n) as u8
}

/// Reduce `region` of an RGBA buffer to `levels` grey levels, in place,
/// diffusing quantization error only inside `dithered`.
///
/// `rgba` covers `width` by `height` pixels with a stride of `width * 4`.
/// Alpha is left alone. Every pixel the region covers comes out with its
/// three colour channels equal, which is what a greyscale or mono packing
/// path expects to find.
///
/// # Why the diffusion is bounded, and by regions rather than by the rect
///
/// Error diffusion is not local. A Floyd–Steinberg pass scoped to an
/// arbitrary damage rect starts from zero error at its edges and leaves a
/// visible seam against the field around it, so a shell that dithers
/// whatever it happened to repaint gets a page with a grid on it.
///
/// The conclusion that follows most easily — only ever diffuse over a
/// whole surface — is too strong, and taking it costs more than the seam
/// did. Diffusion is safe inside a *content-bounded* region, because a
/// photograph's boundary is already a hard content edge and hides the
/// discontinuity the same seam would advertise mid-paragraph. And the
/// alternative to region-bounded diffusion is one flag per surface, which
/// at two levels is decisively wrong in both positions: diffusing through
/// body text stipples the antialiased edge of every glyph, and not
/// diffusing through a photograph turns it into a silhouette.
///
/// So `dithered` is a set of regions and not a boolean. This crate cannot
/// work out what they are — it takes buffers and rects and never a
/// document — so they arrive from the consumer, through
/// [`Update::dithering_within`](crate::Update::dithering_within) or
/// through whatever the consumer runs above the seam.
///
/// Regions are clipped to `region` and to the buffer. Overlapping regions
/// are processed in order and the overlap is diffused twice, which is
/// visible; pass disjoint ones.
pub fn quantize(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    region: PanelRect,
    levels: u8,
    dithered: &[PanelRect],
) {
    let region = region.clamped(width, height);
    if region.is_empty() || width == 0 {
        return;
    }
    let stride = width as usize * 4;
    if rgba.len() < (region.max_y() as usize - 1) * stride + region.max_x() as usize * 4 {
        return;
    }

    // Luminance first, over the whole region, so that both passes below
    // read the same thing and neither has to know whether the other has
    // been past.
    for y in region.y..region.max_y() {
        for x in region.x..region.max_x() {
            let at = y as usize * stride + x as usize * 4;
            let lum = luminance(rgba[at], rgba[at + 1], rgba[at + 2]);
            rgba[at] = lum;
        }
    }

    // Diffused regions first: they read the luminance laid down above and
    // spread error into pixels the plain pass has not touched yet. Doing
    // it the other way round would diffuse already-quantized values.
    for zone in dithered {
        let zone = zone.intersection(region);
        if !zone.is_empty() {
            diffuse(rgba, stride, zone, levels);
        }
    }

    // Everything the diffused zones did not cover, quantized pixel by
    // pixel — and the zones themselves given their sibling channels,
    // which `diffuse` left to be filled in here.
    //
    // This walks *spans*, not pixels, and the difference is not
    // cosmetic. Asking "is this pixel inside a zone?" once per pixel makes
    // the whole page pay per-zone cost to discover that it is not:
    // measured on a 1448x1072 page, that turned a 7ms pass into 70ms
    // with eight zones — an order of magnitude, on the path this feature
    // exists for, inside `submit` and against a 450ms refresh. Stepping to
    // the next zone boundary instead leaves the common inner loop with no
    // zone test in it at all, and a row with no zones over it costs one
    // scan for the whole row.
    for y in region.y..region.max_y() {
        let mut x = region.x;
        while x < region.max_x() {
            let (until, diffused) = match zone_covering(dithered, x, y) {
                Some(end) => (end.min(region.max_x()), true),
                None => (next_zone_start(dithered, x, y, region.max_x()), false),
            };
            for span_x in x..until {
                let at = y as usize * stride + span_x as usize * 4;
                if !diffused {
                    // Inside a zone the level is already there; `diffuse`
                    // wrote it and only the sibling channels are owed.
                    rgba[at] = nearest_level(rgba[at], levels);
                }
                rgba[at + 1] = rgba[at];
                rgba[at + 2] = rgba[at];
            }
            x = until;
        }
    }
}

/// Whether `(x, y)` falls in a diffused zone, and if so where that zone
/// ends — so a caller can step over the whole run at once.
fn zone_covering(dithered: &[PanelRect], x: u32, y: u32) -> Option<u32> {
    dithered
        .iter()
        .find(|zone| {
            !zone.is_empty() && zone.x <= x && x < zone.max_x() && zone.y <= y && y < zone.max_y()
        })
        .map(|zone| zone.max_x())
}

/// The leftmost zone edge strictly right of `x` on row `y`, or `limit`
/// when there is none — the end of the run that needs no zone test.
fn next_zone_start(dithered: &[PanelRect], x: u32, y: u32, limit: u32) -> u32 {
    let mut next = limit;
    for zone in dithered {
        if zone.is_empty() || y < zone.y || y >= zone.max_y() {
            continue;
        }
        if zone.x > x && zone.x < next {
            next = zone.x;
        }
    }
    next
}

/// [`quantize`], taking the request a panel published instead of a level
/// count.
///
/// [`PixelFormat::Rgba`] means *do not reduce*, so this does nothing —
/// that panel reduces for itself, at submit, where it knows the class.
/// `Grey { dither: false }` ignores `dithered`: the flag is the panel's
/// permission and the regions are the consumer's placement, and both have
/// to agree before error goes anywhere.
pub fn quantize_for(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    region: PanelRect,
    format: PixelFormat,
    dithered: &[PanelRect],
) {
    match format {
        PixelFormat::Rgba => {}
        PixelFormat::Grey { levels, dither } => {
            let dithered = if dither { dithered } else { &[][..] };
            quantize(rgba, width, height, region, levels, dithered);
        }
    }
}

/// Floyd–Steinberg over one region, reading and writing the red channel
/// only; the caller equalizes the other two afterwards.
///
/// Error that would leave the region is dropped rather than clamped into
/// the edge pixel. Clamping there is what produces the bright or dark
/// rim that makes a dithered image look pasted on.
fn diffuse(rgba: &mut [u8], stride: usize, zone: PanelRect, levels: u8) {
    // Errors are carried as i16 in a scratch that does not exist: the
    // buffer itself is the scratch, so a pixel's accumulated error is
    // applied by saturating it into the u8 as it is deposited. That costs
    // a little precision at the extremes and saves an allocation, which
    // is the trade that keeps this reachable without `alloc`.
    for y in zone.y..zone.max_y() {
        for x in zone.x..zone.max_x() {
            let at = y as usize * stride + x as usize * 4;
            let old = i16::from(rgba[at]);
            let new = i16::from(nearest_level(rgba[at], levels));
            rgba[at] = new as u8;
            let err = old - new;
            if err == 0 {
                continue;
            }
            let mut deposit = |dx: i64, dy: i64, numerator: i16| {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx < zone.x as i64
                    || nx >= zone.max_x() as i64
                    || ny < zone.y as i64
                    || ny >= zone.max_y() as i64
                {
                    return;
                }
                let to = ny as usize * stride + nx as usize * 4;
                let carried = i16::from(rgba[to]) + err * numerator / 16;
                rgba[to] = carried.clamp(0, 255) as u8;
            };
            deposit(1, 0, 7);
            deposit(-1, 1, 3);
            deposit(0, 1, 5);
            deposit(1, 1, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w` by `h` RGBA buffer from a per-pixel grey value.
    fn grey(w: usize, h: usize, out: &mut [u8], f: impl Fn(usize, usize) -> u8) {
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[v, v, v, 0xFF]);
            }
        }
    }

    fn level_at(rgba: &[u8], w: usize, x: usize, y: usize) -> u8 {
        rgba[(y * w + x) * 4]
    }

    #[test]
    fn white_survives_the_round_trip_at_every_depth() {
        // A page whose white is one step short of white looks washed out
        // and nothing reports it, so the top level has to land exactly.
        for levels in [2u8, 4, 8, 16, 32] {
            assert_eq!(nearest_level(255, levels), 255, "{levels} levels");
            assert_eq!(nearest_level(0, levels), 0, "{levels} levels");
        }
    }

    #[test]
    fn two_levels_threshold_at_the_middle() {
        assert_eq!(nearest_level(127, 2), 0);
        assert_eq!(nearest_level(128, 2), 255);
    }

    #[test]
    fn sixteen_levels_are_evenly_spaced() {
        let mut seen = [0u8; 256];
        let mut count = 0;
        for v in 0..=255u8 {
            let level = nearest_level(v, 16);
            if !seen[..count].contains(&level) {
                seen[count] = level;
                count += 1;
            }
        }
        assert_eq!(count, 16, "got {:?}", &seen[..count]);
        seen[..count].sort_unstable();
        // 17 apart, 0x00 to 0xFF: the classic e-ink ramp.
        for (i, level) in seen[..count].iter().enumerate() {
            assert_eq!(*level, (i * 17) as u8);
        }
    }

    #[test]
    fn luminance_weights_green_most_and_blue_least() {
        assert_eq!(luminance(0, 0, 0), 0);
        assert_eq!(luminance(255, 255, 255), 255);
        assert!(luminance(0, 255, 0) > luminance(255, 0, 0));
        assert!(luminance(255, 0, 0) > luminance(0, 0, 255));
        // Already-grey pixels come through unchanged, so a second pass
        // over a quantized buffer is a no-op rather than a slow drift.
        for level in [0u8, 17, 68, 119, 187, 255] {
            assert_eq!(luminance(level, level, level), level);
        }
    }

    #[test]
    fn quantizing_leaves_the_three_channels_equal() {
        // A greyscale or mono packing path reads one channel and trusts
        // the others; a pixel whose channels disagree packs differently
        // depending on which backend looks at it.
        let mut px = [0u8; 4 * 4 * 4];
        grey(4, 4, &mut px, |x, y| (x * 40 + y * 7) as u8);
        px[0..3].copy_from_slice(&[0x10, 0xC0, 0x40]);
        quantize(&mut px, 4, 4, PanelRect::full(4, 4), 16, &[]);
        for pixel in px.as_chunks::<4>().0 {
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
            assert_eq!(pixel[3], 0xFF, "alpha was touched");
        }
    }

    #[test]
    fn quantizing_touches_only_the_region_it_was_given() {
        // Rule three lives on this: a panel reducing at submit reduces
        // what this update refreshes, not the whole page.
        let mut px = [0u8; 8 * 2 * 4];
        grey(8, 2, &mut px, |_, _| 100);
        quantize(&mut px, 8, 2, PanelRect::new(2, 0, 3, 1), 2, &[]);
        assert_eq!(level_at(&px, 8, 1, 0), 100, "left of the region");
        assert_eq!(level_at(&px, 8, 2, 0), 0, "inside the region");
        assert_eq!(level_at(&px, 8, 4, 0), 0, "inside the region");
        assert_eq!(level_at(&px, 8, 5, 0), 100, "right of the region");
        assert_eq!(level_at(&px, 8, 2, 1), 100, "below the region");
    }

    #[test]
    fn without_a_dither_region_a_flat_mid_grey_goes_solid() {
        // The silhouette failure, stated as a test. Every pixel of a flat
        // 40% grey rounds the same way, so at two levels the whole thing
        // becomes one colour and a photograph loses its shape.
        let mut px = [0u8; 16 * 16 * 4];
        grey(16, 16, &mut px, |_, _| 100);
        quantize(&mut px, 16, 16, PanelRect::full(16, 16), 2, &[]);
        assert!(px.as_chunks::<4>().0.iter().all(|p| p[0] == 0));
    }

    #[test]
    fn inside_a_dither_region_a_flat_mid_grey_keeps_its_tone() {
        // The same field, diffused: the average survives even though
        // every individual pixel is black or white. That is the whole
        // reason the consumer gets to say where its photographs are.
        let mut px = [0u8; 16 * 16 * 4];
        grey(16, 16, &mut px, |_, _| 100);
        let images = [PanelRect::full(16, 16)];
        quantize(&mut px, 16, 16, PanelRect::full(16, 16), 2, &images);

        let lit = px.as_chunks::<4>().0.iter().filter(|p| p[0] == 255).count();
        assert!(
            (60..=100).contains(&lit),
            "{lit} of 256 pixels white; 100/255 of the field is ~100"
        );
        // And it is still two levels, not a smear.
        assert!(px
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[0] == 0 || p[0] == 255));
    }

    #[test]
    fn diffusion_stops_at_the_regions_edge() {
        // Error crossing out of a content-bounded region is the seam this
        // whole shape exists to avoid: outside the image, a flat field
        // must quantize flat.
        let mut px = [0u8; 32 * 8 * 4];
        grey(32, 8, &mut px, |_, _| 100);
        let images = [PanelRect::new(0, 0, 16, 8)];
        quantize(&mut px, 32, 8, PanelRect::full(32, 8), 2, &images);

        for y in 0..8 {
            for x in 16..32 {
                assert_eq!(level_at(&px, 32, x, y), 0, "({x}, {y}) took stray error");
            }
        }
        assert!(
            (0..8).any(|y| level_at(&px, 32, 15, y) == 255),
            "the region itself did not dither"
        );
    }

    #[test]
    fn a_dither_region_outside_the_update_is_ignored() {
        let mut px = [0u8; 16 * 4 * 4];
        grey(16, 4, &mut px, |_, _| 100);
        // The image is on another part of the page than this update.
        let images = [PanelRect::new(12, 0, 4, 4)];
        quantize(&mut px, 16, 4, PanelRect::new(0, 0, 8, 4), 2, &images);
        for y in 0..4 {
            for x in 0..8 {
                assert_eq!(level_at(&px, 16, x, y), 0);
            }
        }
    }

    #[test]
    fn a_panel_asking_for_rgba_gets_its_pixels_back_untouched() {
        // "Do not reduce; I will" has to mean it: a consumer that runs
        // this for every panel must not flatten one that reduces per
        // update.
        let mut px = [0u8; 4 * 4 * 4];
        grey(4, 4, &mut px, |x, y| (x * 40 + y * 7) as u8);
        let before = px;
        quantize_for(
            &mut px,
            4,
            4,
            PanelRect::full(4, 4),
            PixelFormat::Rgba,
            &[PanelRect::full(4, 4)],
        );
        assert_eq!(px, before);
    }

    #[test]
    fn a_panel_that_did_not_ask_for_dithering_does_not_get_it() {
        // The flag is the panel's permission and the regions are the
        // consumer's placement; diffusing on the strength of one alone
        // stipples a page the panel wanted plain.
        let mut px = [0u8; 16 * 16 * 4];
        grey(16, 16, &mut px, |_, _| 100);
        quantize_for(
            &mut px,
            16,
            16,
            PanelRect::full(16, 16),
            PixelFormat::Grey {
                levels: 2,
                dither: false,
            },
            &[PanelRect::full(16, 16)],
        );
        assert!(px.as_chunks::<4>().0.iter().all(|p| p[0] == 0));
    }

    #[test]
    fn a_buffer_shorter_than_its_geometry_is_left_alone() {
        // Geometry and allocation can disagree; running off the end is a
        // fault rather than a wrong pixel.
        let mut px = [0u8; 4 * 4];
        quantize(&mut px, 8, 8, PanelRect::full(8, 8), 2, &[]);
        assert_eq!(px, [0u8; 16]);
    }
}
