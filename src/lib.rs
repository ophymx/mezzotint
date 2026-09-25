//! Getting rendered pixels onto e-ink panels and Linux framebuffers,
//! correctly.
//!
//! An electrophoretic panel does not present frames; it is *asked* to
//! change, and how it is asked decides how the change looks. The same
//! pixels can arrive as a 100ms two-level flicker or a one-second
//! black-white flash, and picking wrong is the difference between a device
//! that feels responsive and one that does not. This crate owns that
//! conversation: a vendor-neutral update vocabulary, a driver that
//! enforces the rules which work on a desk and fail on a device, and the
//! backends underneath.
//!
//! It is that layer and only that layer. Pixels come from the consumer —
//! there is no renderer here, no fonts, no drawing — and input, frontlight
//! and power are somebody else's crate. Nothing above [`Panel`] knows what
//! a page is.
//!
//! The driver and the recording panel need `alloc`, so this example is
//! only compiled when it is on:
//!
#![cfg_attr(feature = "alloc", doc = "```")]
#![cfg_attr(not(feature = "alloc"), doc = "```ignore")]
//! use mezzotint::{PanelDriver, PanelInfo, PanelRect, PixelFormat, RecordingPanel};
//! use mezzotint::{Source, UpdateClass};
//!
//! # fn main() -> mezzotint::Result<()> {
//! let info = PanelInfo::new(400, 300, PixelFormat::Rgba);
//! let mut driver = PanelDriver::new(RecordingPanel::new(info));
//!
//! let page = vec![0xFFu8; info.buffer_len()];
//! let src = Source::panel(&page, info.width, info.height)?;
//!
//! // A page turn: full fidelity, no flash unless the debt has come due.
//! driver.present(src, None, UpdateClass::Quality)?;
//!
//! // A selection tracking under a finger: fast and ugly, and the driver
//! // remembers it owes the region a clean pass.
//! let line = PanelRect::new(0, 40, 400, 20);
//! driver.present(src, Some(line), UpdateClass::Monochrome)?;
//! assert_eq!(driver.provisional(), Some(line));
//!
//! // The finger lifted. Only the shell knows that.
//! driver.settle(src, &[])?;
//! driver.flush()?;
//! # Ok(())
//! # }
//! ```
//!
//! # The rules it holds
//!
//! Each of these is a thing an application otherwise reimplements from
//! folklore, and each is easier to get wrong than to notice.
//!
//! - **[Update classes](UpdateClass), not waveforms.** The consumer names
//!   the *kind* of change; the backend names the waveform. Escalating is
//!   always safe.
//! - **[blit](Panel::blit), [submit](Panel::submit) and [wait](Panel::wait)
//!   are three operations.** A refresh takes 100ms to a second; folding
//!   the wait into the submit is what makes a page turn feel broken.
//! - **Depth belongs to the update, not the session.** Reduction happens
//!   at submit, the first moment the class is known — see
//!   [`PanelInfo::format`].
//! - **[Rects](PanelRect) round outward and may be widened, never
//!   narrowed.** A region trimmed by half a pixel leaves a stale sliver,
//!   and on e-ink a stale sliver stays until something else disturbs it.
//! - **Nothing writes under a live update**, and more than one update can
//!   be live. [`PanelDriver`] tracks the set and waits on genuine overlap
//!   only.
//! - **Fast updates leave debt.** [`PanelDriver::settle`] repays it when
//!   the shell says the gesture ended; [`RefreshPolicy`] rations the flash.
//! - **Where error diffusion belongs is the consumer's to say.** It is not
//!   local and it is not global — see [`Update::dithering_within`].
//!
//! Everything above the ioctls is testable with no hardware attached:
//! [`RecordingPanel`], [`SimulatedPanel`] and the driver's own tests are
//! the conformance suite, and they run on a build machine with no panel.
//! The simulated panel keeps updates in flight on a clock the test owns,
//! and reports from its own side whether anything wrote under one.
//!
//! # Cargo features
//!
//! | feature | what it adds |
//! |---|---|
//! | `alloc` | [`PanelDriver`], [`RecordingPanel`] and [`SimulatedPanel`] |
//! | `std` (default) | `std::io::Error` inside [`Error`], and backends |
//! | `fbdev` (default) | the Linux framebuffer backend, on Linux |
//! | `mxcfb` | the i.MX EPDC ioctl vocabulary, [`mxcfb::uapi`] — not yet a backend |
//!
//! The vocabulary — [`UpdateClass`], [`Update`], [`PanelRect`],
//! [`PanelInfo`], [`Panel`], [`RefreshPolicy`], [`encode`] — needs neither.
//! Turn everything off and it builds for a microcontroller.
//!
//! `fbdev` is additionally gated on `target_os = "linux"`, so leaving it
//! enabled costs nothing on a platform that has no framebuffer: the module
//! is simply absent rather than a build failure.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_debug_implementations)]
#![warn(missing_docs)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod encode;
mod error;
mod panel;
mod policy;
mod rect;
mod source;
mod update;

#[cfg(feature = "alloc")]
mod driver;
#[cfg(feature = "alloc")]
mod recording;
#[cfg(feature = "alloc")]
mod simulated;

// The feature alone is not the gate. `libc` is a Linux-only dependency in
// the manifest, and this module is nothing but ioctls and `/dev/fb0`, so
// on any other target it must vanish rather than fail to compile — which
// is what it did, all sixteen errors of it, because `fbdev` is on by
// default and every CI target here is Linux. A macOS or Windows consumer
// would have hit that on their first build. A backend not existing on a
// platform that has no framebuffer is not a surprise worth an error.
#[cfg(all(feature = "fbdev", target_os = "linux"))]
#[cfg_attr(docsrs, doc(cfg(all(feature = "fbdev", target_os = "linux"))))]
pub mod fbdev;

// Not a backend, and not gated on the target: it is structs, constants
// and the arithmetic that turns them into ioctl numbers, which compiles
// and is worth checking everywhere. The backend that will use it is gated
// on hardware, not on a feature — see `CONTRIBUTING.md`.
#[cfg(feature = "mxcfb")]
#[cfg_attr(docsrs, doc(cfg(feature = "mxcfb")))]
pub mod mxcfb;

pub use error::{Error, Result};
pub use panel::{Panel, PanelInfo, PixelFormat, UpdateToken};
pub use policy::RefreshPolicy;
pub use rect::PanelRect;
pub use source::Source;
pub use update::{Update, UpdateClass, Wait};

#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
pub use driver::{LiveUpdate, PanelDriver};
#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
pub use recording::{Recorded, RecordingPanel};
#[cfg(feature = "alloc")]
#[cfg_attr(docsrs, doc(cfg(feature = "alloc")))]
pub use simulated::{Fault, InFlight, SimConfig, SimulatedPanel, Submitted};
