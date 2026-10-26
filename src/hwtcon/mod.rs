//! A [`Panel`] over MediaTek's `hwtcon` e-ink controller: the first
//! backend here with an update that is genuinely in flight.
//!
//! `hwtcon` is the framebuffer driver on the MediaTek MT8113-family
//! e-readers — Kobo's Clara BW, Clara Colour, Libra Colour and Elipsa 2E
//! and their Tolino twins, per [`devices`](crate::devices). It presents an
//! ordinary `/dev/fb0` with the panel's pixels in it, plus a family of
//! ioctls that look like `mxcfb`'s and mostly carry its numbers: an update
//! is a struct handed to one call and completion is a marker handed to
//! another. [`uapi`] transcribes them.
//!
//! This backend is written against a device, as `CONTRIBUTING.md` asks.
//! The struct layouts, the ioctl numbers — including the one the header
//! gets wrong — and the timings quoted below were all read back from a
//! Kobo Clara Colour (device 393, `spaColour`) running firmware 4.46 on
//! kernel 4.9.
//!
//! # What the controller does for us
//!
//! The framebuffer is 32-bit colour, and the controller reduces it: on a
//! submit it copies the region out through MediaTek's MDP, converts to
//! luminance, runs the colour-filter-array pass on a colour panel, and
//! dithers to the waveform's depth if asked. So this panel asks for
//! [`PixelFormat::Rgba`] and does no reduction in `blit` — the fbdev
//! packing underneath is all the conversion there is — and the per-update
//! depth the contract talks about is handled where it belongs, by the
//! hardware, at the point the class is known.
//!
//! Temperature is not the controller's here, which is the one place this
//! driver differs from what `CONTRIBUTING.md` assumed. The header offers a
//! `TEMP_USE_SENSOR` sentinel, but the driver on the device stores
//! whatever `HWTCON_SET_TEMPERATURE` is handed as *the* panel temperature
//! and indexes the waveform tables by it; the sentinel is honoured only
//! through a debug interface, and the driver's own periodic sensor read is
//! compiled out. The reader that ships with the device sets the
//! temperature from user space, and a shell that replaces that reader
//! owes the panel the same: read a sensor and call
//! [`set_temperature`](HwtconPanel::set_temperature). The driver's own
//! `/proc/hwtcon/temp` only echoes the stored value back; on the Clara
//! Colour the `mtktsAP` thermal zone tracked what the stock reader had
//! set, and the battery charger reports a temperature too. This backend never
//! sets it on its own — a wrong number is worse than a stale one, and
//! while the stock reader is running the number is its.
//!
//! The region is in *framebuffer* coordinates. The panel is mounted
//! landscape and the framebuffer is portrait (`rotate = 3` on the Clara
//! Colour), and the driver rotates the rect itself, so [`PanelInfo`]
//! describes the framebuffer and a consumer never sees the panel's native
//! axes.
//!
//! # Mapping the classes
//!
//! [`Waveforms`] is the table, and these are its defaults. The timings are
//! from `examples/bringup.rs --drive` on a Clara Colour: a 400x400 region in
//! the middle of the page, driven through [`PanelDriver`](crate::PanelDriver)
//! with the stock reader still running, from the call to `present` to the
//! wait returning:
//!
//! | class | waveform | update mode | on glass |
//! |---|---|---|---|
//! | [`Monochrome`](UpdateClass::Monochrome) | `A2`, colour pass skipped | partial | 170–185 ms |
//! | [`Fast`](UpdateClass::Fast) | `DU` | partial | 270–295 ms |
//! | [`Quality`](UpdateClass::Quality) | `GLR16` (REAGL) | partial | 405 ms |
//! | [`Flash`](UpdateClass::Flash) | `GC16` | full | 415–445 ms |
//!
//! `GL16` for the quality class, the plain non-flashing mode, came in at
//! 445 ms on the same card, so REAGL costs nothing here and is the
//! default. The 30–90 ms between `present` and the kernel accepting the
//! update is the blit of the region plus the controller's own intake;
//! submitting the same region straight to the ioctl takes under 5 ms once
//! the controller is awake, so that intake is the first thing a shell that
//! finds a page turn slow should look at.
//!
//! # Dithering
//!
//! The rule from `CONTRIBUTING.md` is applied literally. No diffusion
//! regions: the controller quantizes pixel by pixel. A region that covers
//! the whole rect being refreshed: the controller dithers it, with the
//! `_S` algorithm the driver itself defaults to, at the depth of the
//! waveform. Regions that cover only part of the rect: the hardware
//! ditherer cannot be scoped, so [`encode::quantize`]
//! runs over the staged pixels inside the regions before the update is
//! sent, and the hardware is told to leave the result alone. That last
//! path needs the mapping to be a tightly packed RGBA buffer, which it is
//! on every device this driver runs on; a layout that is not falls back to
//! quantize-only rather than dithering text.
//!
//! On a colour panel there is one more condition. [`encode::quantize`]
//! writes luminance, and the controller's colour-filter pass reads the
//! colour it would have erased — so software diffusion over a rect that
//! holds an illustration would hand the controller a grey one, where the
//! hardware path keeps the colour. On a panel where that pass runs (every
//! hwtcon device with a 32-bit framebuffer and a colour-filter mode set,
//! which is what [`cfa_mode`](HwtconPanel::cfa_mode) reports) the software
//! path is taken only for [`Monochrome`](UpdateClass::Monochrome), whose
//! updates skip the pass anyway. A partial region in any other class gets
//! quantize-only there: diffusion is given up rather than colour.
//! Per-channel diffusion inside the regions would restore it, and is the
//! follow-up.
//!
//! # Waiting
//!
//! The kernel's wait blocks for up to five seconds and cannot be given a
//! shorter deadline, and [`Panel::wait`] promises one. So each submitted
//! update gets a thread that performs the blocking wait on a duplicated
//! descriptor and reports back on a channel; `wait` then honours any
//! timeout, including zero, against that channel. A thread per update is
//! cheap at e-ink cadence, and it is what makes
//! [`PanelDriver::flush_within`](crate::PanelDriver::flush_within) mean
//! what it says on this device.
//!
//! Markers are one namespace in the kernel: a single list for every
//! process that has the device open, and the stock reader counts from one
//! in it. A wait on a marker the kernel does not know returns at once, and
//! a wait on someone else's returns when *theirs* lands — either of which
//! would be a `Complete` for an update still in flight, the one thing the
//! seam exists to rule out. So this backend's markers live in a block of
//! their own: the high half is the process id with its top bit forced on,
//! the low half counts and is never zero.
//!
//! # Provenance
//!
//! Transcribed from `include/linux/hwtcon_ioctl_cmd.h` in the kernel
//! source Kobo publishes for its MT8113 models: `kobolabs/Kobo-Reader` on
//! GitHub, `hw/mt8113-libraC_vision7`, which the Clara Colour's own
//! `hw/mt8113-claraC_shine5C` points at; kernel 4.9.77, fetched
//! 2026-10-25. The header is MediaTek's, under plain GPL-2.0, and unlike
//! `mxcfb.h` it lives in `include/linux` rather than `include/uapi`, so it
//! carries no user-space marking of its own — though it is the interface
//! the device's own reader speaks. What is taken from it is that
//! interface: struct layouts, constant values and the names user space
//! needs to use them, each checked against the running kernel. Those are
//! facts about the device, not anyone's expression of them; every comment
//! in [`uapi`] is this crate's own, and the kernel's `COPYING` says in its
//! first paragraph that user programs using kernel services by normal
//! system calls are not derived works. The driver sources beside the
//! header were read to learn how it behaves — the temperature quirk above
//! comes from them — and nothing in them was copied.
//!
//! [`PanelInfo`]: crate::PanelInfo
//! [`PixelFormat::Rgba`]: crate::PixelFormat::Rgba
//! [`UpdateClass::Monochrome`]: crate::UpdateClass::Monochrome
//! [`UpdateClass::Fast`]: crate::UpdateClass::Fast
//! [`UpdateClass::Quality`]: crate::UpdateClass::Quality
//! [`UpdateClass::Flash`]: crate::UpdateClass::Flash

pub mod uapi;

use std::collections::HashMap;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::fbdev::format::Encoding;
use crate::fbdev::FbdevPanel;
use crate::{
    encode, Error, Panel, PanelInfo, PanelRect, Result, Source, Update, UpdateClass, UpdateToken,
    Wait,
};
use uapi::*;

/// What the driver's framebuffer calls itself in `fb_fix_screeninfo.id`.
const DRIVER_ID: &str = "hwtcon";

/// Which of the controller's waveforms each [`UpdateClass`] uses.
///
/// Public fields, because a shell bringing up a model this crate has not
/// seen is going to need to change one — the numbers are the header's
/// (`WAVEFORM_MODE_*` in [`uapi`]) and the driver maps them through the
/// panel's own waveform file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Waveforms {
    /// For [`UpdateClass::Monochrome`]. Default `A2`.
    pub monochrome: u32,
    /// For [`UpdateClass::Fast`]. Default `DU`.
    pub fast: u32,
    /// For [`UpdateClass::Quality`]. Default `GLR16`, the ghost-compensated
    /// non-flashing mode; `GL16` is the plain one.
    pub quality: u32,
    /// For [`UpdateClass::Flash`], always sent in full update mode. Default
    /// `GC16`.
    pub flash: u32,
}

impl Default for Waveforms {
    fn default() -> Self {
        Waveforms {
            monochrome: WAVEFORM_MODE_A2,
            fast: WAVEFORM_MODE_DU,
            quality: WAVEFORM_MODE_GLR16,
            flash: WAVEFORM_MODE_GC16,
        }
    }
}

/// How one update will be sent, decided before anything is touched.
///
/// Separated from the ioctl so that the decision — the only part of
/// `submit` that can be wrong in a way a test can see — is checkable
/// without a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Plan {
    waveform_mode: u32,
    update_mode: u32,
    flags: u32,
    dither_mode: i32,
    /// Diffuse in software inside the regions first, to this many levels.
    software_levels: Option<u8>,
}

/// Levels the waveform for this class can show, which is what both the
/// hardware and the software ditherer need to be told.
const fn levels_for(class: UpdateClass) -> u8 {
    match class {
        UpdateClass::Monochrome | UpdateClass::Fast => 2,
        UpdateClass::Quality | UpdateClass::Flash => 16,
    }
}

fn plan(
    class: UpdateClass,
    waveforms: &Waveforms,
    rect: PanelRect,
    regions: &[PanelRect],
    software_possible: bool,
) -> Plan {
    let (waveform_mode, update_mode, mut flags) = match class {
        // Two levels and the colour-filter pass has nothing to add, so
        // skip it: the pixels are going to be black or white either way.
        UpdateClass::Monochrome => (
            waveforms.monochrome,
            UPDATE_MODE_PARTIAL,
            HWTCON_FLAG_CFA_SKIP,
        ),
        UpdateClass::Fast => (waveforms.fast, UPDATE_MODE_PARTIAL, 0),
        UpdateClass::Quality => (waveforms.quality, UPDATE_MODE_PARTIAL, 0),
        UpdateClass::Flash => (waveforms.flash, UPDATE_MODE_FULL, 0),
    };
    let levels = levels_for(class);
    let hardware_mode = if levels == 2 {
        HWTCON_FLAG_USE_DITHERING_Y8_Y1_S
    } else {
        HWTCON_FLAG_USE_DITHERING_Y8_Y4_S
    };

    let touching = regions.iter().any(|r| r.intersects(rect));
    let covering = regions.iter().any(|r| r.contains(rect));
    let (dither_mode, software_levels) = if !touching {
        (0, None)
    } else if covering {
        flags |= HWTCON_FLAG_USE_DITHERING;
        (hardware_mode, None)
    } else if software_possible {
        (0, Some(levels))
    } else {
        // Hardware dithering would stipple whatever text shares the rect,
        // which is the outcome the regions exist to prevent; quantize-only
        // is the lesser wrong.
        (0, None)
    };

    Plan {
        waveform_mode,
        update_mode,
        flags,
        dither_mode,
        software_levels,
    }
}

/// A MediaTek `hwtcon` e-ink panel, through its framebuffer.
///
/// Open it with [`open`](HwtconPanel::open), drive it through
/// [`Panel`] — normally behind a [`PanelDriver`](crate::PanelDriver) — and
/// ask it [`panel_info`](HwtconPanel::panel_info) for what the glass is.
#[derive(Debug)]
pub struct HwtconPanel {
    fb: FbdevPanel,
    waveforms: Waveforms,
    /// Whether the mapping is a tightly packed RGBA buffer, which is what
    /// `encode::quantize` can run over in place.
    software_dither: bool,
    /// Whether the controller runs its colour-filter pass, which software
    /// diffusion would defeat; see the module docs.
    colour: bool,
    next_marker: u32,
    /// Updates whose completion has not been collected, by marker.
    live: HashMap<u32, Waiter>,
}

/// One submitted update, as seen from `wait`.
#[derive(Debug)]
enum Waiter {
    /// The thread doing the kernel's wait has not reported.
    Pending(Receiver<std::io::Result<()>>),
    /// It reported an error, which is kept until `wait` asks: a consumer
    /// that submits again before waiting must not lose it to `reap`.
    Failed(std::io::Error),
}

/// Where this process's markers start: the process id, top bit on, in
/// the high half, so that they share nothing with a counter from one.
fn marker_base() -> u32 {
    ((std::process::id() & 0x7FFF) | 0x8000) << 16
}

/// Whether an update may be diffused in software: the mapping has to be
/// RGBA this backend can write, and the controller must not be about to
/// do colour with it, since `encode::quantize` writes luminance.
const fn software_allowed(rgba_mapping: bool, colour: bool, class: UpdateClass) -> bool {
    rgba_mapping && (!colour || matches!(class, UpdateClass::Monochrome))
}

impl HwtconPanel {
    /// Open the default framebuffer, `/dev/fb0`, and check it is this
    /// driver's.
    pub fn open() -> Result<Self> {
        HwtconPanel::open_path("/dev/fb0")
    }

    /// Open a specific framebuffer device and check it is this driver's.
    ///
    /// A framebuffer that is not `hwtcon` is refused with
    /// [`Error::UnexpectedDriver`] rather than driven blind: the ioctls
    /// below share their letter and most of their numbers with `mxcfb`, so
    /// the failure on the wrong device would be a misread update, not an
    /// error.
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        let fb = FbdevPanel::open_path(path)?;
        if fb.driver_id() != DRIVER_ID {
            return Err(Error::UnexpectedDriver {
                expected: DRIVER_ID,
                found: fb.driver_id_bytes(),
            });
        }
        let layout = fb.layout();
        let software_dither = layout.bits_per_pixel == 32
            && layout.line_length == layout.width as usize * 4
            && matches!(
                layout.encoding,
                Encoding::Channels { red, green, blue }
                    if red.offset == 0 && red.length == 8
                        && green.offset == 8 && green.length == 8
                        && blue.offset == 16 && blue.length == 8
            );
        let mut panel = HwtconPanel {
            fb,
            waveforms: Waveforms::default(),
            software_dither,
            colour: false,
            next_marker: marker_base() | 1,
            live: HashMap::new(),
        };
        // The controller does colour when the framebuffer is not 8-bit
        // grey and a colour-filter mode is set — the driver's own test
        // (`hwtcon_core_use_cfa_color_mapping`) minus the per-update flags.
        // If it cannot be asked, a 32-bit framebuffer is taken to be
        // colour: wrong that way costs a photo its diffusion, the other
        // way its colour.
        panel.colour = layout.bits_per_pixel != 8 && panel.cfa_mode().is_none_or(|mode| mode > 0);
        Ok(panel)
    }

    /// Use a different waveform table.
    pub fn with_waveforms(mut self, waveforms: Waveforms) -> Self {
        self.waveforms = waveforms;
        self
    }

    /// The waveform table in use.
    pub fn waveforms(&self) -> Waveforms {
        self.waveforms
    }

    /// What the driver knows about the glass: the waveform file, which
    /// names the panel, plus VCOM and temperature.
    pub fn panel_info(&self) -> Result<hwtcon_panel_info> {
        let mut info = hwtcon_panel_info::default();
        self.ioctl(HWTCON_GET_PANEL_INFO, &mut info, "HWTCON_GET_PANEL_INFO")?;
        Ok(info)
    }

    /// Tell the driver the panel's temperature, in degrees Celsius.
    ///
    /// The waveform tables are indexed by it, and on this driver nothing
    /// else sets it: see the [module docs](self). A shell that owns the
    /// device reads a sensor and calls this on a schedule of minutes; one
    /// running beside the stock reader leaves it alone, because the
    /// reader is already doing so.
    pub fn set_temperature(&self, celsius: i32) -> Result<()> {
        let mut temp = celsius;
        self.ioctl(HWTCON_SET_TEMPERATURE, &mut temp, "HWTCON_SET_TEMPERATURE")
    }

    /// The temperature, in degrees Celsius, the waveform tables are
    /// currently indexed by.
    pub fn temperature(&self) -> Result<i32> {
        let mut temp = 0i32;
        self.ioctl(HWTCON_GET_TEMPERATURE, &mut temp, "HWTCON_GET_TEMPERATURE")?;
        Ok(temp)
    }

    /// The controller's colour-filter mode, as `HWTCON_GET_CFA_MODE`
    /// reports it: zero on a panel with no colour filter, and the mode the
    /// driver is converting through otherwise.
    ///
    /// `None` when the driver will not say, which the backend treats as
    /// colour on a 32-bit framebuffer.
    pub fn cfa_mode(&self) -> Option<u32> {
        let mut mode = 0u32;
        self.ioctl(HWTCON_GET_CFA_MODE, &mut mode, "HWTCON_GET_CFA_MODE")
            .ok()
            .map(|()| mode)
    }

    /// A description of the device, for a shell that wants to say what it
    /// found.
    pub fn describe(&self) -> String {
        format!(
            "{} ({}, {}), waveforms mono {} fast {} quality {} flash {}",
            self.fb.describe(),
            DRIVER_ID,
            if self.colour { "colour" } else { "grey" },
            self.waveforms.monochrome,
            self.waveforms.fast,
            self.waveforms.quality,
            self.waveforms.flash
        )
    }

    /// Read one pixel back as the device spells it; see
    /// [`FbdevPanel::read_pixel`].
    pub fn read_pixel(&self, x: u32, y: u32) -> Option<u32> {
        self.fb.read_pixel(x, y)
    }

    fn fd(&self) -> RawFd {
        self.fb.raw_fd()
    }

    fn ioctl<T>(&self, request: u32, arg: &mut T, doing: &'static str) -> Result<()> {
        // Safety: every request here takes a pointer to a struct of the
        // size its number encodes, and `arg` is exactly that struct.
        let r = unsafe { libc::ioctl(self.fd(), request as _, arg as *mut T) };
        if r < 0 {
            return Err(Error::Io {
                doing,
                source: std::io::Error::last_os_error(),
            });
        }
        Ok(())
    }

    /// Drop every update whose waiter has already reported, so the set
    /// stays the size of what is actually in flight even for a consumer
    /// that never waits.
    fn reap(&mut self) {
        self.live.retain(|_, waiter| {
            let outcome = match waiter {
                Waiter::Failed(_) => return true,
                Waiter::Pending(rx) => rx.try_recv(),
            };
            match outcome {
                Err(mpsc::TryRecvError::Empty) => true,
                Ok(Err(source)) => {
                    *waiter = Waiter::Failed(source);
                    true
                }
                Ok(Ok(())) | Err(mpsc::TryRecvError::Disconnected) => false,
            }
        });
    }

    fn mint(&mut self) -> u32 {
        let marker = self.next_marker;
        // Count in the low half only, and never through zero: the driver
        // treats a marker of 0 as "none", and the high half is the block
        // that keeps these apart from every other process's.
        let next = (marker.wrapping_add(1) & 0xFFFF).max(1);
        self.next_marker = (marker & 0xFFFF_0000) | next;
        marker
    }
}

impl Panel for HwtconPanel {
    fn info(&self) -> PanelInfo {
        self.fb.info()
    }

    fn blit(&mut self, src: Source<'_>, rect: PanelRect) -> Result<()> {
        self.fb.blit(src, rect)
    }

    fn submit(&mut self, rect: PanelRect, update: Update<'_>) -> Result<UpdateToken> {
        self.reap();
        let info = self.info();
        let rect = rect.clamped(info.width, info.height);
        let marker = self.mint();
        if rect.is_empty() {
            // Nothing to ask the controller for; the token is complete
            // the moment it exists.
            return Ok(UpdateToken::new(marker));
        }

        let plan = plan(
            update.class(),
            &self.waveforms,
            rect,
            update.dither_regions(),
            software_allowed(self.software_dither, self.colour, update.class()),
        );
        if let Some(levels) = plan.software_levels {
            let (width, height) = (info.width, info.height);
            encode::quantize(
                self.fb.mapping(),
                width,
                height,
                rect,
                levels,
                update.dither_regions(),
            );
        }

        let mut data = hwtcon_update_data {
            update_region: hwtcon_rect {
                top: rect.y,
                left: rect.x,
                width: rect.w,
                height: rect.h,
            },
            waveform_mode: plan.waveform_mode,
            update_mode: plan.update_mode,
            update_marker: marker,
            flags: plan.flags,
            dither_mode: plan.dither_mode,
        };
        // The kernel's wait has no deadline of ours, so it runs on its own
        // thread against its own descriptor, and `wait` consults the
        // channel. The dup keeps the device open for the thread however
        // the panel is dropped.
        //
        // The waiter is set up *before* anything is sent, so a failure to
        // dup or to spawn is a submit that never happened rather than an
        // update in flight with nobody watching it — the driver would
        // rewind its cadence on the error and write under the refresh.
        // The thread starts its wait only once told the update went in;
        // if it is never told, it closes up and leaves.
        //
        // Safety: `dup` on a descriptor we hold open; the copy is closed
        // by the thread that owns it, or below if the thread never runs.
        let dup = unsafe { libc::dup(self.fd()) };
        if dup < 0 {
            return Err(Error::Io {
                doing: "dup framebuffer for wait",
                source: std::io::Error::last_os_error(),
            });
        }
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("hwtcon-wait".into())
            .spawn(move || {
                if go_rx.recv().is_ok() {
                    let mut md = hwtcon_update_marker_data {
                        update_marker: marker,
                        collision_test: 0,
                    };
                    // Safety: the request's payload is this struct; `dup`
                    // is a descriptor this thread owns until the close
                    // below.
                    let r =
                        unsafe { libc::ioctl(dup, HWTCON_WAIT_FOR_UPDATE_COMPLETE as _, &mut md) };
                    let result = if r < 0 {
                        Err(std::io::Error::last_os_error())
                    } else {
                        Ok(())
                    };
                    // The receiver may be gone if the panel was dropped;
                    // that is fine, nobody is waiting.
                    let _ = tx.send(result);
                }
                // Safety: closing the descriptor this thread owns.
                unsafe {
                    libc::close(dup);
                }
            });
        if let Err(source) = spawned {
            // Safety: the thread never started, so the dup is ours to close.
            unsafe {
                libc::close(dup);
            }
            return Err(Error::Io {
                doing: "spawn hwtcon wait thread",
                source,
            });
        }

        // An error here drops `go_tx`, and the thread closes its
        // descriptor and exits without ever having waited for anything.
        self.ioctl(HWTCON_SEND_UPDATE, &mut data, "HWTCON_SEND_UPDATE")?;
        let _ = go_tx.send(());
        self.live.insert(marker, Waiter::Pending(rx));
        Ok(UpdateToken::new(marker))
    }

    fn wait(&mut self, token: UpdateToken, timeout: Option<Duration>) -> Result<Wait> {
        let marker = token.raw();
        if let Some(Waiter::Failed(_)) = self.live.get(&marker) {
            if let Some(Waiter::Failed(source)) = self.live.remove(&marker) {
                return Err(Error::Io {
                    doing: "HWTCON_WAIT_FOR_UPDATE_COMPLETE",
                    source,
                });
            }
        }
        let Some(Waiter::Pending(rx)) = self.live.get(&marker) else {
            return Ok(Wait::Complete);
        };
        let outcome = match timeout {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(t) => rx.recv_timeout(t),
        };
        match outcome {
            Ok(Ok(())) => {
                self.live.remove(&marker);
                Ok(Wait::Complete)
            }
            Ok(Err(source)) => {
                self.live.remove(&marker);
                Err(Error::Io {
                    doing: "HWTCON_WAIT_FOR_UPDATE_COMPLETE",
                    source,
                })
            }
            // The waiter is gone without a word, which only a panic in
            // it could cause; the kernel's own bound has long passed by
            // then, so the region is not still being read.
            Err(RecvTimeoutError::Disconnected) => {
                self.live.remove(&marker);
                Ok(Wait::Complete)
            }
            Err(RecvTimeoutError::Timeout) => Ok(Wait::TimedOut),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: Waveforms = Waveforms {
        monochrome: WAVEFORM_MODE_A2,
        fast: WAVEFORM_MODE_DU,
        quality: WAVEFORM_MODE_GLR16,
        flash: WAVEFORM_MODE_GC16,
    };
    const RECT: PanelRect = PanelRect::new(100, 100, 400, 400);

    #[test]
    fn software_diffusion_yields_to_the_colour_pass() {
        // A grey panel with an RGBA mapping may always diffuse in software.
        assert!(software_allowed(true, false, UpdateClass::Quality));
        // A colour panel only where the pass is skipped anyway.
        assert!(software_allowed(true, true, UpdateClass::Monochrome));
        assert!(!software_allowed(true, true, UpdateClass::Quality));
        assert!(!software_allowed(true, true, UpdateClass::Fast));
        assert!(!software_allowed(true, true, UpdateClass::Flash));
        // And never without a mapping it can write.
        assert!(!software_allowed(false, false, UpdateClass::Quality));
    }

    #[test]
    fn markers_stay_in_this_process_block_and_never_hit_zero() {
        let base = marker_base();
        assert_ne!(base & 0x8000_0000, 0, "top bit on, clear of small counters");
        assert_eq!(base & 0xFFFF, 0);
        // Walk the low half round once, the way `mint` does.
        let mut marker = base | 0xFFFE;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(marker);
            let next = (marker.wrapping_add(1) & 0xFFFF).max(1);
            marker = (marker & 0xFFFF_0000) | next;
        }
        assert_eq!(seen, [base | 0xFFFE, base | 0xFFFF, base | 1]);
    }

    #[test]
    fn the_classes_map_to_the_waveforms_in_the_table() {
        let p = plan(UpdateClass::Monochrome, &W, RECT, &[], true);
        assert_eq!(p.waveform_mode, WAVEFORM_MODE_A2);
        assert_eq!(p.update_mode, UPDATE_MODE_PARTIAL);
        assert_eq!(p.flags, HWTCON_FLAG_CFA_SKIP);
        assert_eq!(
            plan(UpdateClass::Fast, &W, RECT, &[], true).waveform_mode,
            WAVEFORM_MODE_DU
        );
        assert_eq!(
            plan(UpdateClass::Quality, &W, RECT, &[], true).waveform_mode,
            WAVEFORM_MODE_GLR16
        );
    }

    #[test]
    fn only_flash_is_a_full_update() {
        // `update_mode` is the class, not a second axis: a full-mode
        // update of anything but the flash waveform would flash anyway.
        for class in [
            UpdateClass::Monochrome,
            UpdateClass::Fast,
            UpdateClass::Quality,
        ] {
            assert_eq!(
                plan(class, &W, RECT, &[], true).update_mode,
                UPDATE_MODE_PARTIAL,
                "{class:?}"
            );
        }
        let p = plan(UpdateClass::Flash, &W, RECT, &[], true);
        assert_eq!(p.update_mode, UPDATE_MODE_FULL);
        assert_eq!(p.waveform_mode, WAVEFORM_MODE_GC16);
    }

    #[test]
    fn no_regions_means_no_dithering_anywhere() {
        let p = plan(UpdateClass::Quality, &W, RECT, &[], true);
        assert_eq!(p.flags & HWTCON_FLAG_USE_DITHERING, 0);
        assert_eq!(p.dither_mode, 0);
        assert_eq!(p.software_levels, None);
        // A region elsewhere on the page is the same as none here.
        let elsewhere = [PanelRect::new(600, 600, 50, 50)];
        let p = plan(UpdateClass::Quality, &W, RECT, &elsewhere, true);
        assert_eq!(p.flags & HWTCON_FLAG_USE_DITHERING, 0);
        assert_eq!(p.software_levels, None);
    }

    #[test]
    fn a_region_covering_the_rect_uses_the_hardware_at_the_waveforms_depth() {
        let whole = [PanelRect::new(0, 0, 1072, 1448)];
        let p = plan(UpdateClass::Quality, &W, RECT, &whole, true);
        assert_ne!(p.flags & HWTCON_FLAG_USE_DITHERING, 0);
        assert_eq!(p.dither_mode, HWTCON_FLAG_USE_DITHERING_Y8_Y4_S);
        assert_eq!(p.software_levels, None);
        let p = plan(UpdateClass::Fast, &W, RECT, &whole, true);
        assert_eq!(p.dither_mode, HWTCON_FLAG_USE_DITHERING_Y8_Y1_S);
    }

    #[test]
    fn a_region_inside_the_rect_diffuses_in_software_and_tells_the_hardware_to_leave_it() {
        let photo = [PanelRect::new(150, 150, 100, 100)];
        let p = plan(UpdateClass::Quality, &W, RECT, &photo, true);
        assert_eq!(p.flags & HWTCON_FLAG_USE_DITHERING, 0);
        assert_eq!(p.dither_mode, 0);
        assert_eq!(p.software_levels, Some(16));
        assert_eq!(
            plan(UpdateClass::Monochrome, &W, RECT, &photo, true).software_levels,
            Some(2)
        );
    }

    #[test]
    fn without_a_packed_mapping_a_partial_region_falls_back_to_quantize_only() {
        // Dithering the whole rect would stipple the text around the
        // photograph, which is exactly what the regions were for.
        let photo = [PanelRect::new(150, 150, 100, 100)];
        let p = plan(UpdateClass::Quality, &W, RECT, &photo, false);
        assert_eq!(p.flags & HWTCON_FLAG_USE_DITHERING, 0);
        assert_eq!(p.software_levels, None);
    }

    #[test]
    fn the_default_table_is_the_measured_one() {
        assert_eq!(Waveforms::default(), W);
    }
}
