//! Regions of a panel, in whole panel pixels.

/// A rectangle in whole panel pixels, in the panel's own orientation.
///
/// # Rounding is the caller's, and it rounds outward
///
/// A consumer's damage arithmetic is in floating point — glyph geometry,
/// CSS pixels, a scale factor — and panel controllers take integer
/// regions, so something has to round. Whatever does it must round
/// **outward**: a region trimmed by half a pixel leaves a stale sliver on
/// screen, and on e-ink a stale sliver stays there until something else
/// disturbs it. Growing a region costs time; shrinking one costs
/// correctness, permanently.
///
/// The conversion itself is not here on purpose. Mapping a page rect onto
/// a panel needs the consumer's own coordinate space and its own idea of
/// rotation, and a constructor here could only serve one of them. This
/// crate takes buffers and rects and never a document — so the rounding
/// rule is stated here, where every backend can read it, and applied on
/// the consumer's side, where the types are.
///
/// # A rect is a minimum
///
/// A rect handed to [`Panel::submit`](crate::Panel::submit) says what must
/// be refreshed, not what may be. See that method for what a backend owes
/// when it takes more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PanelRect {
    /// Leftmost column.
    pub x: u32,
    /// Topmost row.
    pub y: u32,
    /// Width in pixels. Zero makes the rect empty.
    pub w: u32,
    /// Height in pixels. Zero makes the rect empty.
    pub h: u32,
}

impl PanelRect {
    /// A region `w` by `h` with its top-left corner at `(x, y)`.
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        PanelRect { x, y, w, h }
    }

    /// The whole panel.
    pub const fn full(width: u32, height: u32) -> Self {
        PanelRect::new(0, 0, width, height)
    }

    /// A region with no pixels in it. Covers nothing, intersects nothing,
    /// and is contained by everything.
    pub const fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// One past the rightmost column, saturating: a rect whose right edge
    /// would exceed `u32::MAX` is already past any real panel, and
    /// wrapping here would make it look like it started before the left
    /// edge — which turns an out-of-range region into a silently valid
    /// one.
    pub const fn max_x(self) -> u32 {
        self.x.saturating_add(self.w)
    }

    /// One past the bottom row, saturating for the same reason as
    /// [`max_x`](PanelRect::max_x).
    pub const fn max_y(self) -> u32 {
        self.y.saturating_add(self.h)
    }

    /// How many pixels this region covers.
    pub const fn area(self) -> u64 {
        self.w as u64 * self.h as u64
    }

    /// Whether two regions share a pixel.
    ///
    /// A panel must not be written to while an update covering the same
    /// pixels is still in flight, so this is the test a driver uses to
    /// decide whether it has to wait. Edges are exclusive: rects that
    /// merely touch share nothing.
    pub const fn intersects(self, other: PanelRect) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.x < other.max_x()
            && other.x < self.max_x()
            && self.y < other.max_y()
            && other.y < self.max_y()
    }

    /// Whether `other` lies entirely inside this region. An empty region
    /// is contained by anything and contains nothing.
    pub const fn contains(self, other: PanelRect) -> bool {
        if other.is_empty() {
            return true;
        }
        !self.is_empty()
            && self.x <= other.x
            && self.y <= other.y
            && self.max_x() >= other.max_x()
            && self.max_y() >= other.max_y()
    }

    /// The smallest region covering both. An empty operand contributes
    /// nothing.
    pub const fn union(self, other: PanelRect) -> PanelRect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = if self.x < other.x { self.x } else { other.x };
        let y = if self.y < other.y { self.y } else { other.y };
        let right = if self.max_x() > other.max_x() {
            self.max_x()
        } else {
            other.max_x()
        };
        let bottom = if self.max_y() > other.max_y() {
            self.max_y()
        } else {
            other.max_y()
        };
        PanelRect::new(x, y, right - x, bottom - y)
    }

    /// The region covered by both, empty when they do not overlap.
    ///
    /// Backends want this where a widened region has to be trimmed back
    /// to what actually exists — a bus window clipped to the panel, a
    /// damage rect clipped to a mapping.
    pub const fn intersection(self, other: PanelRect) -> PanelRect {
        if !self.intersects(other) {
            return PanelRect::new(0, 0, 0, 0);
        }
        let x = if self.x > other.x { self.x } else { other.x };
        let y = if self.y > other.y { self.y } else { other.y };
        let right = if self.max_x() < other.max_x() {
            self.max_x()
        } else {
            other.max_x()
        };
        let bottom = if self.max_y() < other.max_y() {
            self.max_y()
        } else {
            other.max_y()
        };
        PanelRect::new(x, y, right - x, bottom - y)
    }

    /// This region clipped to a panel `width` by `height`.
    pub const fn clamped(self, width: u32, height: u32) -> PanelRect {
        self.intersection(PanelRect::full(width, height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersects_is_exclusive_at_the_edge() {
        let a = PanelRect::new(0, 0, 10, 10);
        assert!(a.intersects(PanelRect::new(9, 9, 5, 5)));
        // Touching edges share no pixel.
        assert!(!a.intersects(PanelRect::new(10, 0, 5, 5)));
        assert!(!a.intersects(PanelRect::new(0, 10, 5, 5)));
        assert!(!a.intersects(PanelRect::new(0, 0, 0, 0)));
    }

    #[test]
    fn union_ignores_empty_operands() {
        let a = PanelRect::new(5, 5, 10, 10);
        assert_eq!(a.union(PanelRect::default()), a);
        assert_eq!(PanelRect::default().union(a), a);
        assert_eq!(
            a.union(PanelRect::new(20, 0, 5, 5)),
            PanelRect::new(5, 0, 20, 15)
        );
    }

    #[test]
    fn containment_treats_empty_as_nothing_to_own() {
        let a = PanelRect::new(0, 0, 10, 10);
        assert!(a.contains(PanelRect::new(2, 2, 4, 4)));
        assert!(a.contains(a));
        assert!(!a.contains(PanelRect::new(2, 2, 20, 4)));
        assert!(a.contains(PanelRect::default()));
        assert!(!PanelRect::default().contains(a));
    }

    #[test]
    fn intersection_of_disjoint_regions_is_empty() {
        let a = PanelRect::new(0, 0, 10, 10);
        assert_eq!(
            a.intersection(PanelRect::new(4, 4, 20, 20)),
            PanelRect::new(4, 4, 6, 6)
        );
        assert!(a.intersection(PanelRect::new(10, 0, 5, 5)).is_empty());
        assert!(a.intersection(PanelRect::default()).is_empty());
    }

    #[test]
    fn clamping_keeps_a_rect_on_the_panel() {
        let r = PanelRect::new(90, 190, 50, 50).clamped(100, 200);
        assert_eq!(r, PanelRect::new(90, 190, 10, 10));
        assert!(PanelRect::new(200, 0, 10, 10).clamped(100, 200).is_empty());
    }

    #[test]
    fn a_rect_that_would_overflow_stays_past_the_edge() {
        // Saturating rather than wrapping: a rect whose right edge exceeds
        // u32 must still read as "starts at x and goes on", not as one
        // that ended before it began, or a bounds test would pass it.
        let huge = PanelRect::new(u32::MAX - 4, 0, 100, 10);
        assert_eq!(huge.max_x(), u32::MAX);
        assert!(huge.intersects(PanelRect::new(u32::MAX - 2, 0, 1, 10)));
        assert!(huge.clamped(100, 200).is_empty());
    }
}
