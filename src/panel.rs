//! The seam: what a panel is, and the three things you can ask one to do.

use core::time::Duration;

use crate::{PanelRect, Result, Source, Update, Wait};

/// What a panel wants *produced for it*.
///
/// A request, not a description of the hardware. A greyscale framebuffer
/// showing 256 levels and an e-ink controller that dithers in hardware
/// both ask for [`PixelFormat::Rgba`], and mean different things by it;
/// what they have in common is that reducing above them would throw
/// something away for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PixelFormat {
    /// **Do not reduce; I will.** The default, and right for more panels
    /// than it first looks:
    ///
    /// - A controller that quantizes and dithers in hardware. Reducing
    ///   first spends a pass over a multi-megabyte buffer to pre-flatten
    ///   what the controller would flatten anyway, and differently.
    /// - A greyscale framebuffer, whose `blit` reduces to luminance on the
    ///   way in, so quantizing above it only discards precision.
    /// - A panel whose depth depends on the update rather than the
    ///   session — which is every EPDC. See [`Panel`].
    #[default]
    Rgba,
    /// Luminance quantized to `levels` steps: 2 for a 1-bit panel, 4 and
    /// 16 for the shallow and full waveforms of a monochrome e-ink one.
    ///
    /// No upper bound is imposed. A cap here would be a claim about what
    /// panels exist rather than a property of the arithmetic. Fewer than
    /// two levels is not a picture, so that floor stays.
    ///
    /// `dither` says diffusion is *permitted*, not that it is wanted
    /// everywhere — see [`Update::dithering_within`] for where. Asking for
    /// this rather than `Rgba` is right only when the reduction genuinely
    /// has to happen above the panel and one answer serves every update,
    /// which is the case for a panel with a single fixed depth and no
    /// hardware to do it.
    Grey {
        /// How many evenly spaced grey values the panel can show.
        levels: u8,
        /// Whether error diffusion is permitted at all.
        dither: bool,
    },
}

/// What a panel is, as far as anything above it needs to know.
///
/// Deliberately *not* a rotation. A device mounts its panel in a fixed
/// orientation, and turning a page to match is a property of the pixels
/// on the way in, decided in one place on the consumer's side. A second
/// field here meaning nearly the same thing would leave nothing to say
/// which wins. `width` and `height` describe the panel as the controller
/// addresses it, and the buffer handed to [`Panel::blit`] is already in
/// that orientation.
///
/// Public fields plus [`new`](PanelInfo::new), rather than
/// `#[non_exhaustive]`: backends live outside this crate and *build* one
/// of these, so sealing construction would make a third-party backend
/// impossible to write. Adding a field here is a breaking change, and
/// that is the trade taken deliberately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelInfo {
    /// Panel width in device pixels, as the controller addresses it.
    pub width: u32,
    /// Panel height in device pixels, as the controller addresses it.
    pub height: u32,
    /// What to produce for this panel, and the authority on the question.
    ///
    /// # Why this survives the encoder moving below the seam
    ///
    /// Once [`encode`](crate::encode) lives in this crate, asking the
    /// consumer to pre-reduce looks like a cost that no longer needs
    /// paying — a full-page quantize is a few milliseconds against a panel
    /// refresh of several hundred. But the reason this exists was never
    /// cost. It is the only channel by which a panel can say *reduce for
    /// me*, and a consumer doing its own reduction is a consumer that
    /// knows which parts of the buffer are photographs and which are
    /// paragraphs — see [`Update::dithering_within`] for why that matters
    /// so much at two levels.
    ///
    /// [`Update::dithering_within`] is the other shape of the same answer,
    /// and the better one: it keeps per-update reduction *and*
    /// region-aware diffusion, which this field cannot do at once. Both
    /// exist because only a backend on real EPDC hardware can show that
    /// the second retires the first.
    pub format: PixelFormat,
}

impl PanelInfo {
    /// Describe a panel of this size that wants pixels in this format.
    pub const fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        PanelInfo {
            width,
            height,
            format,
        }
    }

    /// The whole panel as a rect.
    pub const fn bounds(&self) -> PanelRect {
        PanelRect::full(self.width, self.height)
    }

    /// Bytes an RGBA buffer covering the whole panel takes.
    pub const fn buffer_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

/// A submitted update, returned by [`Panel::submit`] and consumed by
/// [`Panel::wait`].
///
/// Opaque on purpose: on an mxcfb device this is the controller's update
/// marker, and nothing above the panel should care. The field is private
/// so that it can stop being a `u32` — a backend that queues to a
/// different kind of controller has somewhere to go — without breaking
/// every backend on the day it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UpdateToken(u32);

impl UpdateToken {
    /// Mint a token. For backends; a consumer only ever passes one back.
    pub const fn new(marker: u32) -> Self {
        UpdateToken(marker)
    }

    /// The marker this token was minted from — how a backend recovers what
    /// it has to hand the kernel.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// A screen that is asked to change rather than presented to.
///
/// # blit, submit and wait are three operations
///
/// The split is not incidental. On a real controller these are two
/// distinct actions — a memcpy into mapped memory, then an ioctl — and an
/// update takes long enough, 100ms to a second, that folding the wait into
/// the submit makes a page turn feel broken. [`submit`](Panel::submit)
/// returns immediately with a token; the caller decides when it has to
/// know the pixels landed, and usually the answer is "never, until
/// teardown."
///
/// # Never write under a live update
///
/// **Implementors and callers both owe this.** Never [`blit`](Panel::blit)
/// a region while an update covering it is still in flight: the controller
/// is reading that memory, and writing under it tears, or leaves the panel
/// showing a mixture of two frames. [`PanelRect::intersects`] is the test
/// and [`wait`](Panel::wait) is the fix.
///
/// More than one update can be live at a time. A driver that remembers
/// only the most recent one forgets the earlier the moment a
/// non-overlapping update arrives, and the next write over the forgotten
/// region proceeds while the controller is still reading it — which is
/// exactly the tearing this rule exists to prevent.
/// [`PanelDriver`](crate::PanelDriver) tracks the set and is the reason
/// most consumers never implement this rule themselves.
///
/// # Reducing per update
///
/// The depth a panel can show is often a property of the *update*, not of
/// the session: two levels for a fast waveform, four for a shallow one,
/// sixteen for a full one. A single [`PanelInfo::format`] cannot express
/// that, and pre-reducing to any one of them is wrong in both directions —
/// flatten to sixteen and the fast update re-flattens it anyway, flatten
/// to two and the next page turn is ruined.
///
/// Such a panel asks for [`PixelFormat::Rgba`], keeps what `blit` staged,
/// and reduces in `submit`, which is the first point where the class is
/// known. Nothing in this trait needs to change for that: `blit` is
/// defined as *taking* the pixels, not as copying them into a mapping, so
/// storage the panel owns is a legitimate destination — and for a panel
/// reached over a bus it is the only one.
///
/// # Not yet, but not precluded
///
/// A backend may one day hand *out* its staging buffer, so a consumer
/// renders straight into mapped memory and skips a copy. That wants a
/// measurement on real hardware before it is designed, so it is absent —
/// but nothing here forecloses it: it arrives as a provided method
/// returning `Option<&mut [u8]>`, guarded by the in-flight rules above,
/// and adding one to a trait breaks nobody.
pub trait Panel {
    /// Geometry and capabilities. Cheap; callers may ask per frame.
    fn info(&self) -> PanelInfo;

    /// Take `rect` from `src`, converted to whatever this device wants and
    /// held wherever this device holds it.
    ///
    /// Deliberately not "copy into the panel's memory": that is true of a
    /// mapped framebuffer and of a locked platform surface, but a panel on
    /// the far end of a bus has no memory to copy into. There, `blit`
    /// stages and [`submit`](Panel::submit) transmits. The obligation is
    /// only that the pixels are taken by the time this returns, since the
    /// caller may reuse the buffer.
    ///
    /// `src` is RGBA in panel orientation, already quantized to
    /// [`info().format`](PanelInfo::format), and positioned in panel
    /// coordinates — it may cover the whole panel or only the part that
    /// changed. It must cover all of `rect`; [`Source::require`] is that
    /// check, and an implementation owes it before it indexes anything.
    ///
    /// Conversion lives here because only the panel knows its own spelling
    /// — four-bit grey packed two to a byte, RGB565, 1bpp packed,
    /// inverted, whatever.
    fn blit(&mut self, src: Source<'_>, rect: PanelRect) -> Result<()>;

    /// Ask the panel to show *at least* `rect`, using whatever waveform
    /// this device uses for the update's class. Returns as soon as the
    /// request is queued.
    ///
    /// A panel may refresh **more** than it was asked to and must never
    /// refresh less. Hardware routinely forces this: controllers impose
    /// alignment on the region, panels driven over a bus refresh
    /// byte-aligned windows or nothing smaller than the whole screen, and
    /// a flash covers everything by definition. Widening only costs time;
    /// narrowing leaves the screen showing something that is no longer
    /// true, which on e-ink persists.
    ///
    /// A panel that widens owns the consequence. The rule against writing
    /// under a live update is enforced above here against the region that
    /// was *requested*, because that is all a caller knows; a panel that
    /// went further is the only party that knows it did, so it must make
    /// its own [`blit`](Panel::blit) safe against the region it actually
    /// took. On a bus-attached panel that falls out for free — it cannot
    /// transmit while the controller is busy anyway.
    fn submit(&mut self, rect: PanelRect, update: Update<'_>) -> Result<UpdateToken>;

    /// Block until a submitted update has finished reaching the glass, or
    /// until `timeout` passes.
    ///
    /// `None` waits forever. That is the honest default for a controller
    /// that is working, and the reason a timeout exists at all is the one
    /// that is not: mxcfb's `WAIT_FOR_UPDATE_COMPLETE` blocks indefinitely
    /// on a wedged controller, and a trait with no deadline gives a shell
    /// no way out but to hang. A zero timeout polls.
    ///
    /// Waiting on an already-completed or unknown token returns
    /// [`Wait::Complete`] — a caller that lost track should not be
    /// punished for asking.
    ///
    /// [`Wait::TimedOut`] is not an error and does not retire anything.
    /// The update is still live and its region still must not be written.
    fn wait(&mut self, token: UpdateToken, timeout: Option<Duration>) -> Result<Wait>;
}
