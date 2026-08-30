//! The pixels a [`blit`](crate::Panel::blit) is given, and where on the
//! panel they belong.

use crate::{Error, PanelRect, Result};

/// A borrowed view of RGBA pixels positioned in panel coordinates.
///
/// # Why this is not just `&[u8]`
///
/// The obvious convention is the one the first consumer wanted: hand over
/// a buffer covering the whole panel on every call and state the damage
/// separately. That costs a page-oriented reader nothing, because it
/// already keeps exactly one page buffer. It costs an embedded consumer
/// driving a 400x300 panel a whole-panel RGBA allocation — 480 KiB — to
/// update a status line, on a host that may not have it to spare. Since
/// widening this after publication is a breaking change to the one method
/// every backend implements, it is settled here: a source states its own
/// origin, extent and stride, and a full-panel buffer is the special case
/// [`Source::panel`] builds.
///
/// Backends should read through [`row`](Source::row) or
/// [`pixel`](Source::pixel) rather than indexing `pixels` themselves, so
/// that a partial source and a full one take the same path.
///
/// # What this deliberately is not
///
/// Not owned, and not a staging buffer. Whether a backend can hand *out*
/// its own mapped memory for a consumer to render straight into — skipping
/// a copy on a device where the copy is a measurable part of a page turn —
/// is a real question and an unanswered one. It wants a measurement on a
/// real mmap'd framebuffer rather than a guess, and it is deliberately not
/// pre-empted here: the answer arrives later as a provided method on
/// [`Panel`](crate::Panel), which adds nothing breaking when it does.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    pixels: &'a [u8],
    /// Where this buffer's top-left pixel sits on the panel.
    origin: (u32, u32),
    width: u32,
    height: u32,
    /// Bytes from the start of one row to the start of the next. Not
    /// always `width * 4`: a view into a larger buffer keeps that
    /// buffer's stride.
    stride: usize,
}

impl<'a> Source<'a> {
    /// A buffer covering the whole panel, tightly packed.
    ///
    /// The common case, and the one a consumer that keeps a single page
    /// buffer wants. Fails when the buffer is not exactly
    /// `width * height * 4` bytes, which is the check that catches a
    /// buffer built in the wrong orientation before it reaches the glass.
    pub fn panel(pixels: &'a [u8], width: u32, height: u32) -> Result<Self> {
        Source::at(pixels, PanelRect::full(width, height))
    }

    /// A buffer covering exactly `rect`, tightly packed — the sub-buffer
    /// case, for a consumer that renders only what changed.
    pub fn at(pixels: &'a [u8], rect: PanelRect) -> Result<Self> {
        Source::with_stride(pixels, rect, rect.w as usize * 4)
    }

    /// A buffer covering `rect` whose rows are `stride` bytes apart — a
    /// window into someone else's larger buffer, without copying it out.
    pub fn with_stride(pixels: &'a [u8], rect: PanelRect, stride: usize) -> Result<Self> {
        let row_bytes = rect.w as usize * 4;
        // Only `height - 1` full strides are required. The last row needs
        // its pixels, not its padding, and demanding the padding would
        // reject a legitimate window onto the final row of a larger
        // buffer.
        let needed = if rect.is_empty() {
            0
        } else {
            (rect.h as usize - 1) * stride + row_bytes
        };
        if stride < row_bytes || pixels.len() < needed {
            return Err(Error::NotCovered {
                asked: rect,
                given: PanelRect::new(
                    rect.x,
                    rect.y,
                    supplied_w(stride, rect),
                    rows_that_fit(pixels.len(), row_bytes, stride, rect.h),
                ),
            });
        }
        Ok(Source {
            pixels,
            origin: (rect.x, rect.y),
            width: rect.w,
            height: rect.h,
            stride,
        })
    }

    /// The region of the panel these pixels describe.
    pub const fn bounds(&self) -> PanelRect {
        PanelRect::new(self.origin.0, self.origin.1, self.width, self.height)
    }

    /// Bytes from the start of one row of these pixels to the next.
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// The raw bytes, for a backend that has established the shape it
    /// wants and would rather walk them itself.
    pub const fn pixels(&self) -> &'a [u8] {
        self.pixels
    }

    /// Whether these pixels can supply every pixel of `rect`.
    ///
    /// The check a backend owes at the top of `blit`; see
    /// [`require`](Source::require) for the version that produces the
    /// error.
    pub const fn covers(&self, rect: PanelRect) -> bool {
        self.bounds().contains(rect)
    }

    /// [`covers`](Source::covers), as a `Result`.
    pub fn require(&self, rect: PanelRect) -> Result<()> {
        if self.covers(rect) {
            Ok(())
        } else {
            Err(Error::NotCovered {
                asked: rect,
                given: self.bounds(),
            })
        }
    }

    /// The RGBA bytes of panel row `y`, starting at panel column `x` and
    /// running to the right edge of this source.
    ///
    /// `x` and `y` are panel coordinates, not offsets into the buffer, so
    /// a backend iterating a damage rect writes the same loop whether it
    /// was handed the whole panel or a window onto part of it.
    pub fn row(&self, x: u32, y: u32) -> Option<&'a [u8]> {
        let (ox, oy) = self.origin;
        let (dx, dy) = (x.checked_sub(ox)?, y.checked_sub(oy)?);
        if dx >= self.width || dy >= self.height {
            return None;
        }
        let start = dy as usize * self.stride + dx as usize * 4;
        let end = start + (self.width - dx) as usize * 4;
        self.pixels.get(start..end)
    }

    /// One pixel, in panel coordinates.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let row = self.row(x, y)?;
        Some([row[0], row[1], row[2], row[3]])
    }
}

/// How many whole pixels a row of `stride` bytes can hold, capped at what
/// was asked for — what the failed `given` rect reports as its width.
fn supplied_w(stride: usize, rect: PanelRect) -> u32 {
    ((stride / 4) as u64).min(rect.w as u64) as u32
}

/// How many rows of `row_bytes` pixels a buffer of `len` bytes holds at
/// `stride`, capped at what was asked for. The last row needs only its
/// pixels, matching what `with_stride` requires.
fn rows_that_fit(len: usize, row_bytes: usize, stride: usize, wanted: u32) -> u32 {
    if row_bytes == 0 || stride == 0 || len < row_bytes {
        return 0;
    }
    let rows = 1 + (len - row_bytes) / stride;
    (rows as u64).min(wanted as u64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fill `out` with a `w` by `h` ramp: red is the column, green the row.
    fn ramp(w: usize, h: usize, out: &mut [u8]) {
        for y in 0..h {
            for x in 0..w {
                let at = (y * w + x) * 4;
                out[at..at + 4].copy_from_slice(&[x as u8, y as u8, 0, 0xFF]);
            }
        }
    }

    #[test]
    fn a_full_panel_source_addresses_by_panel_coordinates() {
        let mut px = [0u8; 8 * 4 * 4];
        ramp(8, 4, &mut px);
        let src = Source::panel(&px, 8, 4).unwrap();
        assert_eq!(src.bounds(), PanelRect::full(8, 4));
        assert_eq!(src.pixel(3, 2), Some([3, 2, 0, 0xFF]));
        assert_eq!(src.row(6, 1).unwrap().len(), 2 * 4);
        assert_eq!(src.pixel(8, 0), None);
    }

    #[test]
    fn a_sub_buffer_addresses_by_panel_coordinates_too() {
        // The whole point of the convention: an embedded consumer that
        // rendered only its status line hands over only those pixels, and
        // the backend's loop does not change.
        let mut px = [0u8; 4 * 2 * 4];
        ramp(4, 2, &mut px);
        let src = Source::at(&px, PanelRect::new(100, 50, 4, 2)).unwrap();
        assert_eq!(src.pixel(100, 50), Some([0, 0, 0, 0xFF]));
        assert_eq!(src.pixel(103, 51), Some([3, 1, 0, 0xFF]));
        // Outside the window is not "black", it is absent.
        assert_eq!(src.pixel(99, 50), None);
        assert_eq!(src.pixel(104, 50), None);
    }

    #[test]
    fn a_window_onto_a_larger_buffer_follows_its_stride() {
        let mut px = [0u8; 8 * 4 * 4];
        ramp(8, 4, &mut px);
        // The right half of rows 1 and 2, without copying it out.
        let window = &px[(8 + 4) * 4..];
        let src = Source::with_stride(window, PanelRect::new(4, 1, 4, 2), 8 * 4).unwrap();
        assert_eq!(src.pixel(4, 1), Some([4, 1, 0, 0xFF]));
        assert_eq!(src.pixel(7, 2), Some([7, 2, 0, 0xFF]));
    }

    #[test]
    fn the_last_row_need_not_carry_its_padding() {
        // A window onto the bottom row of a larger buffer ends at that
        // row's pixels. Requiring a full final stride would reject it.
        let mut px = [0u8; 8 * 4 * 4];
        ramp(8, 4, &mut px);
        let window = &px[3 * 8 * 4..];
        let src = Source::with_stride(window, PanelRect::new(0, 3, 8, 1), 8 * 4).unwrap();
        assert_eq!(src.pixel(7, 3), Some([7, 3, 0, 0xFF]));
    }

    #[test]
    fn a_buffer_in_the_wrong_orientation_is_refused() {
        // The classic mistake: a portrait page handed to a landscape
        // panel is exactly the right number of bytes with the axes
        // swapped. Without this check it writes plausible garbage.
        let mut px = [0u8; 4 * 8 * 4];
        ramp(4, 8, &mut px);
        let err = Source::panel(&px[..4 * 7 * 4], 8, 4).unwrap_err();
        assert!(matches!(err, Error::NotCovered { .. }), "{err:?}");
        // Same byte count, wrong shape, is the case a length check alone
        // cannot see — so a backend must also state the region it needs.
        let square = Source::panel(&px, 4, 8).unwrap();
        assert!(square.require(PanelRect::full(8, 4)).is_err());
    }

    #[test]
    fn a_source_that_does_not_reach_the_region_is_refused() {
        let mut px = [0u8; 8 * 4 * 4];
        ramp(8, 4, &mut px);
        let src = Source::panel(&px, 8, 4).unwrap();
        assert!(src.require(PanelRect::new(0, 0, 8, 4)).is_ok());
        assert!(src.require(PanelRect::new(4, 2, 4, 2)).is_ok());
        let err = src.require(PanelRect::new(4, 2, 8, 2)).unwrap_err();
        assert!(matches!(err, Error::NotCovered { .. }), "{err:?}");
    }

    #[test]
    fn a_short_buffer_reports_what_it_could_actually_supply() {
        // The error is read on a device with no debugger attached, so it
        // has to say how far the pixels got, not just that they did not.
        let px = [0u8; 8 * 4 * 2];
        let err = Source::panel(&px, 8, 4).unwrap_err();
        match err {
            Error::NotCovered { asked, given } => {
                assert_eq!(asked, PanelRect::full(8, 4));
                assert_eq!(given, PanelRect::new(0, 0, 8, 2));
            }
            other => panic!("{other:?}"),
        }
    }
}
