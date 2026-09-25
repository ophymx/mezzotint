//! What the consumer asks for: the kind of change, and where error
//! diffusion is allowed to run while showing it.

use crate::PanelRect;

/// What a change needs the panel to do in order to show it correctly — the
/// vendor-neutral half of a waveform choice.
///
/// Each variant is the *least disruptive* update that still renders its
/// change faithfully, so the ordering runs from cheapest and ugliest to
/// slowest and cleanest. Escalating is always safe; the picture is right
/// either way, it just costs more and disturbs more of the screen.
///
/// There is no `None`. "Nothing to show" is the absence of an update, not
/// a kind of one, and putting it here would make every backend write a
/// match arm for a case that must never reach it. Where a consumer's own
/// arithmetic can produce "no change", that is an `Option<UpdateClass>` on
/// the consumer's side of the seam.
///
/// # What these usually map to
///
/// Defaults a backend starts from, not API — the constant a controller
/// wants for the same visual result differs per model, which is the whole
/// reason this enum is not a waveform number.
///
/// | class | typical waveform |
/// |---|---|
/// | [`Monochrome`](UpdateClass::Monochrome) | `A2` |
/// | [`Fast`](UpdateClass::Fast) | `DU` |
/// | [`Quality`](UpdateClass::Quality) | `GC16_FAST`, or a Regal mode |
/// | [`Flash`](UpdateClass::Flash) | `GC16`, full update mode |
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UpdateClass {
    /// Two levels, as fast as the controller can go, visible artifacts
    /// acceptable. For changes that are still moving — a selection
    /// following a finger — where latency matters more than fidelity and
    /// the result will be redrawn properly when the gesture ends.
    Monochrome,
    /// Fast, few grey levels, no flash. For small settled changes: a
    /// highlight appearing, a status line ticking over.
    Fast,
    /// The panel's full grey range, without a flash. The normal case for
    /// new content — a page turn, a chapter, an image that finished
    /// decoding.
    Quality,
    /// Full grey range *with* a black/white flash to clear accumulated
    /// ghosting. Slow and visually loud, so it is reserved for moments the
    /// user already expects a beat, or for paying off the ghost debt that
    /// [`RefreshPolicy`](crate::RefreshPolicy) tracks.
    Flash,
}

impl UpdateClass {
    /// Whether the class leaves its region visibly degraded once it
    /// settles, so a driver owes it a full-fidelity pass later.
    ///
    /// Only [`UpdateClass::Monochrome`] qualifies. It drops the region to
    /// two levels, which turns antialiased text into something you would
    /// not want to read once the finger stops moving. `Fast` also loses
    /// grey levels, but stays legible, and cleaning after every settled
    /// highlight would blink the screen more than the ghosting is worth —
    /// [`RefreshPolicy`](crate::RefreshPolicy) catches its slower
    /// accumulation instead.
    pub const fn is_provisional(self) -> bool {
        matches!(self, UpdateClass::Monochrome)
    }

    /// Whether the class is what a gesture in progress produces.
    ///
    /// These never escalate to a flash on cadence: blinking the screen
    /// under a moving finger is worse than any amount of ghosting.
    pub const fn is_in_gesture(self) -> bool {
        matches!(self, UpdateClass::Monochrome | UpdateClass::Fast)
    }
}

/// One submission to a panel: the class, plus the hints a backend may use
/// while reducing for it.
///
/// A struct rather than a bare `class` argument so that a hint can be
/// added without breaking the one trait method every backend implements:
/// fields here are private and grow behind builder methods, and a backend
/// that only reads the class keeps compiling when one arrives.
///
/// A hint earns a place here only when the consumer knows something the
/// backend cannot derive from the class, the rect and the regions below.
/// The fields of `mxcfb_update_data` that look like candidates do not:
/// partial-or-full is the class, the quantization depth is the waveform's,
/// the temperature is the controller's own sensor, and the hardware dither
/// mode follows from the diffusion regions — hardware when they cover the
/// whole refreshed rect, quantize-only when there are none, software
/// inside them otherwise. `CONTRIBUTING.md` records each of those under
/// *Decisions that are settled*, so a backend need not wait for them.
///
/// # Where diffusion is allowed
///
/// [`dithering_within`](Update::dithering_within) is the channel this
/// crate has for a fact it cannot work out for itself. Error diffusion is
/// not local: a Floyd–Steinberg pass scoped to an arbitrary rect starts
/// from zero error at its edges and seams against the field around it. The
/// tempting conclusion — only ever diffuse whole surfaces — is too strong.
/// Diffusion is safe inside a *content-bounded* region, because a
/// photograph's edge is already a hard content edge and hides the
/// discontinuity that the same seam would advertise mid-paragraph.
///
/// Which regions those are is knowledge this crate cannot have. It takes
/// buffers and rects and never a document, so it cannot tell a photograph
/// from a paragraph — and at two levels the difference is decisive:
/// diffusing through body text stipples the antialiased edge of every
/// glyph, and not diffusing through a photograph turns it into a
/// silhouette. So the consumer names the regions and the panel does the
/// reduction, at [`submit`](crate::Panel::submit), which is the first
/// point where the class — and therefore the depth — is known.
///
/// A backend that reduces above itself instead, by asking for pre-reduced
/// pixels through [`PanelInfo::format`](crate::PanelInfo::format), can
/// ignore these; the consumer is doing its own quantization in that case
/// and already knows where its images are. See
/// [`PanelInfo::format`](crate::PanelInfo::format) for why both channels
/// exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Update<'a> {
    class: UpdateClass,
    dither: &'a [PanelRect],
}

impl<'a> Update<'a> {
    /// An update of this class, with no diffusion permitted anywhere.
    pub const fn new(class: UpdateClass) -> Self {
        Update { class, dither: &[] }
    }

    /// Permit error diffusion inside these regions, in panel
    /// coordinates, and nowhere else.
    ///
    /// Regions outside any of them are quantized pixel by pixel. An empty
    /// set — the default — means no diffusion at all, which is the right
    /// answer for a page of text and the wrong one for a photograph.
    pub const fn dithering_within(mut self, regions: &'a [PanelRect]) -> Self {
        self.dither = regions;
        self
    }

    /// What kind of change this is.
    pub const fn class(self) -> UpdateClass {
        self.class
    }

    /// The regions error diffusion may run inside. Empty by default.
    pub const fn dither_regions(self) -> &'a [PanelRect] {
        self.dither
    }

    /// The same update with a different class, keeping every other hint.
    ///
    /// This is how [`RefreshPolicy`](crate::RefreshPolicy) escalates to a
    /// flash without discarding what the consumer said about its images.
    pub const fn with_class(mut self, class: UpdateClass) -> Self {
        self.class = class;
        self
    }
}

impl From<UpdateClass> for Update<'_> {
    fn from(class: UpdateClass) -> Self {
        Update::new(class)
    }
}

/// How a submitted update ended.
///
/// [`Panel::wait`](crate::Panel::wait) returns this rather than treating a
/// deadline as an error: a caller that passed a zero timeout to poll is
/// not in an exceptional state when the answer is "not yet."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[must_use = "a timed-out update is still live, and its region still must not be written"]
pub enum Wait {
    /// The pixels are on the glass. The region is free to write again.
    Complete,
    /// The deadline passed with the update still outstanding. The
    /// controller may still be reading the staged memory, so the region is
    /// *not* free.
    TimedOut,
}

impl Wait {
    /// Whether the pixels are on the glass and the region is free to
    /// write again.
    pub const fn is_complete(self) -> bool {
        matches!(self, Wait::Complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_order_cheapest_to_cleanest() {
        // Escalating is always safe, so the ordering has to mean
        // "at least as thorough as" for a backend that clamps.
        assert!(UpdateClass::Monochrome < UpdateClass::Fast);
        assert!(UpdateClass::Fast < UpdateClass::Quality);
        assert!(UpdateClass::Quality < UpdateClass::Flash);
        assert_eq!(
            UpdateClass::Fast.max(UpdateClass::Flash),
            UpdateClass::Flash
        );
    }

    #[test]
    fn only_monochrome_leaves_debt() {
        assert!(UpdateClass::Monochrome.is_provisional());
        for class in [UpdateClass::Fast, UpdateClass::Quality, UpdateClass::Flash] {
            assert!(!class.is_provisional(), "{class:?}");
        }
    }

    #[test]
    fn escalation_keeps_the_diffusion_hints() {
        // A flash that forgot where the photographs were would re-render
        // the page worse than the update it replaced.
        let images = [PanelRect::new(0, 0, 40, 40)];
        let update = Update::new(UpdateClass::Quality).dithering_within(&images);
        let escalated = update.with_class(UpdateClass::Flash);
        assert_eq!(escalated.class(), UpdateClass::Flash);
        assert_eq!(escalated.dither_regions(), &images);
    }

    #[test]
    fn diffusion_is_off_unless_asked_for() {
        // The default has to be "nowhere": dithering a page of text
        // stipples every glyph edge, and a backend that guessed would be
        // guessing on every update a consumer did not think about.
        assert!(Update::new(UpdateClass::Quality)
            .dither_regions()
            .is_empty());
    }
}
