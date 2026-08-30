//! What can go wrong between a buffer and the glass.

use core::fmt;

use crate::{PanelRect, UpdateToken};

/// The result of anything that talks to a panel.
pub type Result<T> = core::result::Result<T, Error>;

/// A failure on the way to the glass.
///
/// Deliberately small, and deliberately structured rather than stringly
/// typed. A backend runs on a device with no terminal attached, so the
/// numbers a diagnosis needs — the depth the kernel reported, the visual
/// it claimed — have to survive being formatted, and a `String` would
/// also put the whole vocabulary behind `alloc` for the sake of error
/// text.
///
/// `#[non_exhaustive]`: backends live outside this crate, but they only
/// ever *construct* these. A consumer matching on one is matching on a set
/// that will grow as backends do.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// [`blit`](crate::Panel::blit) was handed pixels that do not cover
    /// the region it was told to take.
    ///
    /// The common cause is a buffer built for the wrong orientation: a
    /// portrait page handed to a landscape panel is the right number of
    /// bytes with the axes swapped, and without this check it writes
    /// plausible garbage rather than failing.
    NotCovered {
        /// The region `blit` was asked for.
        asked: PanelRect,
        /// The region the pixels actually span, in panel coordinates.
        given: PanelRect,
    },

    /// The device spells its pixels in a way the backend cannot write.
    ///
    /// `visual` is the kernel's `fb_fix_screeninfo.visual` where there is
    /// one; `reason` says which property was the problem. Both are kept
    /// because on an unfamiliar panel the numbers *are* the diagnosis.
    UnsupportedPixels {
        /// The depth the device reported.
        bits_per_pixel: u32,
        /// The kernel's `fb_fix_screeninfo.visual`, or 0 where the device
        /// has no such notion.
        visual: u32,
        /// Which property was the problem.
        reason: &'static str,
    },

    /// The device reported a geometry nothing can be drawn into.
    UnsupportedGeometry {
        /// The width the device reported.
        width: u32,
        /// The height the device reported.
        height: u32,
    },

    /// A [`wait`](crate::Panel::wait) with no deadline came back without
    /// the update having completed.
    ///
    /// Only a backend that breaks the contract produces this. It is an
    /// error rather than a shrug because the alternative is to carry on
    /// and write under an update that is still live, which is the one
    /// thing the driver exists to prevent.
    WaitStalled {
        /// The update that did not finish.
        token: UpdateToken,
    },

    /// The operating system refused something on the way to the panel.
    ///
    /// `doing` names the call, because `ENOTTY` from an ioctl and
    /// `ENOTTY` from an open are different bugs.
    #[cfg(feature = "std")]
    Io {
        /// The call that failed.
        doing: &'static str,
        /// What the operating system said.
        source: std::io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotCovered { asked, given } => write!(
                f,
                "pixels covering {}x{}+{}+{} cannot supply the region {}x{}+{}+{}",
                given.w, given.h, given.x, given.y, asked.w, asked.h, asked.x, asked.y
            ),
            Error::UnsupportedPixels {
                bits_per_pixel,
                visual,
                reason,
            } => write!(
                f,
                "{bits_per_pixel} bits per pixel, visual {visual}: {reason}"
            ),
            Error::UnsupportedGeometry { width, height } => {
                write!(f, "device reports a {width}x{height} screen")
            }
            Error::WaitStalled { token } => write!(
                f,
                "update {} did not complete on an unbounded wait",
                token.raw()
            ),
            #[cfg(feature = "std")]
            Error::Io { doing, source } => write!(f, "{doing}: {source}"),
        }
    }
}

impl core::error::Error for Error {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            #[cfg(feature = "std")]
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
