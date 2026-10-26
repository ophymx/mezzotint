//! `include/uapi/linux/mxcfb.h`, the EPDC half, transcribed.
//!
//! Names are the header's own, C spelling and all, so that a reader can
//! diff this against the source line by line; see the
//! [module docs](super) for the citation and for why the ioctl numbers are
//! derived rather than written down.
//!
//! Only the EPDC interface is here. The header also carries the IPU
//! overlay, alpha, gamma and colour-key ioctls for the LCD side of the
//! same driver, and those have nothing to do with an e-ink panel.
//!
//! ```
//! use mezzotint::mxcfb::uapi::*;
//!
//! let update = mxcfb_update_data {
//!     update_region: mxcfb_rect { top: 40, left: 0, width: 600, height: 20 },
//!     waveform_mode: WAVEFORM_MODE_AUTO,
//!     update_mode: UPDATE_MODE_PARTIAL,
//!     update_marker: 1,
//!     temp: TEMP_USE_AMBIENT,
//!     flags: 0,
//!     dither_mode: EPDC_FLAG_USE_DITHERING_PASSTHROUGH,
//!     quant_bit: 0,
//!     alt_buffer_data: mxcfb_alt_buffer_data::default(),
//! };
//! assert_eq!(update.update_region.width, 600);
//! assert_eq!(MXCFB_SEND_UPDATE, 0x4048_462E);
//! ```

#![allow(non_camel_case_types)]

use core::mem::size_of;

// The `_IO*` encoding lives in `crate::ioctl` so that every transcribed
// header derives its numbers the same way; it is re-exported here because
// this module published it first.
pub use crate::ioctl::{io, ior, iow, iowr};

// ---------------------------------------------------------------------
// include/uapi/linux/mxcfb.h
// ---------------------------------------------------------------------

/// A region of the panel, in the controller's own order: top before left.
///
/// Not [`PanelRect`](crate::PanelRect), and not convertible by accident.
/// The field order is the kernel's, `x, y` here is `left, top`, and a
/// backend does the translation in one place where it can be read.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct mxcfb_rect {
    /// First row.
    pub top: u32,
    /// First column.
    pub left: u32,
    /// Columns.
    pub width: u32,
    /// Rows.
    pub height: u32,
}

/// The framebuffer's pixels are 8-bit grey.
pub const GRAYSCALE_8BIT: u32 = 0x1;
/// The framebuffer's pixels are 8-bit grey, 0 white.
pub const GRAYSCALE_8BIT_INVERTED: u32 = 0x2;
/// The framebuffer's pixels are 4-bit grey.
pub const GRAYSCALE_4BIT: u32 = 0x3;
/// The framebuffer's pixels are 4-bit grey, 0 white.
pub const GRAYSCALE_4BIT_INVERTED: u32 = 0x4;

/// `MXCFB_SET_AUTO_UPDATE_MODE`: updates happen only when asked for.
pub const AUTO_UPDATE_MODE_REGION_MODE: u32 = 0;
/// `MXCFB_SET_AUTO_UPDATE_MODE`: the driver refreshes whatever changes.
pub const AUTO_UPDATE_MODE_AUTOMATIC_MODE: u32 = 1;

/// `MXCFB_SET_UPDATE_SCHEME`: the pixels are captured at submit.
pub const UPDATE_SCHEME_SNAPSHOT: u32 = 0;
/// `MXCFB_SET_UPDATE_SCHEME`: updates queue and run in order.
pub const UPDATE_SCHEME_QUEUE: u32 = 1;
/// `MXCFB_SET_UPDATE_SCHEME`: queued updates that overlap are merged.
pub const UPDATE_SCHEME_QUEUE_AND_MERGE: u32 = 2;

/// Only pixels that changed are driven.
pub const UPDATE_MODE_PARTIAL: u32 = 0x0;
/// Every pixel in the region is driven, which with a 16-level waveform is
/// the flash.
pub const UPDATE_MODE_FULL: u32 = 0x1;

/// REAGL, the ghost-compensating 16-level waveform.
pub const WAVEFORM_MODE_GLR16: u32 = 4;
/// REAGL with dithering.
pub const WAVEFORM_MODE_GLD16: u32 = 5;
/// Let the driver pick a waveform from the pixels it was given.
pub const WAVEFORM_MODE_AUTO: u32 = 257;

/// `temp`: use the controller's own temperature sensor.
pub const TEMP_USE_AMBIENT: i32 = 0x1000;

/// Invert the pixels on the way to the panel.
pub const EPDC_FLAG_ENABLE_INVERSION: u32 = 0x01;
/// Threshold the pixels to black and white on the way to the panel.
pub const EPDC_FLAG_FORCE_MONOCHROME: u32 = 0x02;
/// Pass the pixels through the framebuffer's colour map.
pub const EPDC_FLAG_USE_CMAP: u32 = 0x04;
/// Read the pixels from `alt_buffer_data` rather than the framebuffer.
pub const EPDC_FLAG_USE_ALT_BUFFER: u32 = 0x100;
/// Do not update; report through `collision_test` whether one would
/// collide with an update in flight.
pub const EPDC_FLAG_TEST_COLLISION: u32 = 0x200;
/// Hold this update until the group is released.
pub const EPDC_FLAG_GROUP_UPDATE: u32 = 0x400;
/// Older spelling of hardware dithering to one bit; superseded by
/// `dither_mode` and `quant_bit`.
pub const EPDC_FLAG_USE_DITHERING_Y1: u32 = 0x2000;
/// Older spelling of hardware dithering to four bits; superseded by
/// `dither_mode` and `quant_bit`.
pub const EPDC_FLAG_USE_DITHERING_Y4: u32 = 0x4000;
/// Use the REAGL waveform where the driver supports it.
pub const EPDC_FLAG_USE_REGAL: u32 = 0x8000;

/// `dither_mode`: no hardware dithering.
pub const EPDC_FLAG_USE_DITHERING_PASSTHROUGH: i32 = 0x0;
/// `dither_mode`: Floyd–Steinberg, over the whole update region.
pub const EPDC_FLAG_USE_DITHERING_FLOYD_STEINBERG: i32 = 0x1;
/// `dither_mode`: Atkinson, over the whole update region.
pub const EPDC_FLAG_USE_DITHERING_ATKINSON: i32 = 0x2;
/// `dither_mode`: an ordered pattern, over the whole update region.
pub const EPDC_FLAG_USE_DITHERING_ORDERED: i32 = 0x3;
/// `dither_mode`: quantize to `quant_bit` bits without diffusing.
pub const EPDC_FLAG_USE_DITHERING_QUANT_ONLY: i32 = 0x4;
/// One past the last valid `dither_mode`.
pub const EPDC_FLAG_USE_DITHERING_MAX: i32 = 0x5;

/// `MXCFB_SET_PWRDOWN_DELAY`: never power the controller down.
pub const FB_POWERDOWN_DISABLE: i32 = -1;

/// Where to read pixels from instead of the framebuffer, when
/// [`EPDC_FLAG_USE_ALT_BUFFER`] is set.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct mxcfb_alt_buffer_data {
    /// Physical address of the buffer.
    pub phys_addr: u32,
    /// Width of the entire buffer.
    pub width: u32,
    /// Height of the entire buffer.
    pub height: u32,
    /// Region within the buffer to update.
    pub alt_update_region: mxcfb_rect,
}

/// One update request: the payload of [`MXCFB_SEND_UPDATE`].
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct mxcfb_update_data {
    /// The region to refresh.
    pub update_region: mxcfb_rect,
    /// Which waveform, in this vendor's numbering.
    pub waveform_mode: u32,
    /// [`UPDATE_MODE_PARTIAL`] or [`UPDATE_MODE_FULL`].
    pub update_mode: u32,
    /// The marker [`MXCFB_WAIT_FOR_UPDATE_COMPLETE`] will wait on. Zero
    /// means the caller does not intend to wait.
    pub update_marker: u32,
    /// Panel temperature in degrees Celsius, or [`TEMP_USE_AMBIENT`].
    pub temp: i32,
    /// Any of the `EPDC_FLAG_*` bits.
    pub flags: u32,
    /// One of the `EPDC_FLAG_USE_DITHERING_*` modes.
    pub dither_mode: i32,
    /// Bits per pixel to quantize to when `dither_mode` is not
    /// passthrough.
    pub quant_bit: i32,
    /// Only read when [`EPDC_FLAG_USE_ALT_BUFFER`] is set.
    pub alt_buffer_data: mxcfb_alt_buffer_data,
}

/// The payload of [`MXCFB_WAIT_FOR_UPDATE_COMPLETE`].
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct mxcfb_update_marker_data {
    /// The marker from the update being waited on.
    pub update_marker: u32,
    /// Written by the kernel: whether the update collided with another.
    pub collision_test: u32,
}

/// The payload of [`MXCFB_SET_WAVEFORM_MODES`]: which of the vendor's
/// waveform numbers the driver should use for each purpose when it
/// selects one itself.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct mxcfb_waveform_modes {
    /// Clearing the panel to white.
    pub mode_init: i32,
    /// Direct update, black and white.
    pub mode_du: i32,
    /// Four grey levels.
    pub mode_gc4: i32,
    /// Eight grey levels.
    pub mode_gc8: i32,
    /// Sixteen grey levels.
    pub mode_gc16: i32,
    /// Thirty-two grey levels.
    pub mode_gc32: i32,
}

/// Tell the driver which waveform numbers to use for auto-selection.
pub const MXCFB_SET_WAVEFORM_MODES: u32 = iow::<mxcfb_waveform_modes>(b'F', 0x2B);
/// Override the temperature the waveform tables are indexed by.
pub const MXCFB_SET_TEMPERATURE: u32 = iow::<i32>(b'F', 0x2C);
/// One of the `AUTO_UPDATE_MODE_*` values.
pub const MXCFB_SET_AUTO_UPDATE_MODE: u32 = iow::<u32>(b'F', 0x2D);
/// Submit an update.
pub const MXCFB_SEND_UPDATE: u32 = iow::<mxcfb_update_data>(b'F', 0x2E);
/// Block until the update with a given marker has reached the glass.
pub const MXCFB_WAIT_FOR_UPDATE_COMPLETE: u32 = iowr::<mxcfb_update_marker_data>(b'F', 0x2F);
/// Milliseconds of idleness before the controller powers down, or
/// [`FB_POWERDOWN_DISABLE`].
pub const MXCFB_SET_PWRDOWN_DELAY: u32 = iow::<i32>(b'F', 0x30);
/// Read the power-down delay back.
pub const MXCFB_GET_PWRDOWN_DELAY: u32 = ior::<i32>(b'F', 0x31);
/// One of the `UPDATE_SCHEME_*` values.
pub const MXCFB_SET_UPDATE_SCHEME: u32 = iow::<u32>(b'F', 0x32);
/// The physical address of the controller's working buffer.
///
/// The payload is `unsigned long`, so this is the one number here that
/// differs between a 32-bit and a 64-bit kernel.
pub const MXCFB_GET_WORK_BUFFER: u32 = iowr::<usize>(b'F', 0x34);
/// Stop the controller reading the framebuffer.
pub const MXCFB_DISABLE_EPDC_ACCESS: u32 = io(b'F', 0x35);
/// Let the controller read the framebuffer again.
pub const MXCFB_ENABLE_EPDC_ACCESS: u32 = io(b'F', 0x36);

/// Every field in the header is `__u32` or `int`, so these layouts are
/// the same on both arms — and the assertions are unconditional so that
/// the armv7 and aarch64 checks in CI each evaluate them, rather than
/// trusting that they would agree.
///
/// The two ioctl numbers other implementations publish are pinned here as
/// well: they are the only independent check on the `_IOW` encoding
/// above, since everything else is derived from it.
mod layout_matches_the_kernel {
    use super::*;
    use core::mem::offset_of;

    const _: () = {
        assert!(size_of::<mxcfb_rect>() == 16);
        assert!(offset_of!(mxcfb_rect, top) == 0);
        assert!(offset_of!(mxcfb_rect, left) == 4);
        assert!(offset_of!(mxcfb_rect, width) == 8);
        assert!(offset_of!(mxcfb_rect, height) == 12);

        assert!(size_of::<mxcfb_alt_buffer_data>() == 28);
        assert!(offset_of!(mxcfb_alt_buffer_data, phys_addr) == 0);
        assert!(offset_of!(mxcfb_alt_buffer_data, alt_update_region) == 12);

        assert!(size_of::<mxcfb_update_data>() == 72);
        assert!(offset_of!(mxcfb_update_data, update_region) == 0);
        assert!(offset_of!(mxcfb_update_data, waveform_mode) == 16);
        assert!(offset_of!(mxcfb_update_data, update_mode) == 20);
        assert!(offset_of!(mxcfb_update_data, update_marker) == 24);
        assert!(offset_of!(mxcfb_update_data, temp) == 28);
        assert!(offset_of!(mxcfb_update_data, flags) == 32);
        assert!(offset_of!(mxcfb_update_data, dither_mode) == 36);
        assert!(offset_of!(mxcfb_update_data, quant_bit) == 40);
        assert!(offset_of!(mxcfb_update_data, alt_buffer_data) == 44);

        assert!(size_of::<mxcfb_update_marker_data>() == 8);
        assert!(offset_of!(mxcfb_update_marker_data, collision_test) == 4);

        assert!(size_of::<mxcfb_waveform_modes>() == 24);
        assert!(offset_of!(mxcfb_waveform_modes, mode_gc32) == 20);

        assert!(MXCFB_SEND_UPDATE == 0x4048_462E);
        assert!(MXCFB_WAIT_FOR_UPDATE_COMPLETE == 0xC008_462F);
    };

    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(MXCFB_GET_WORK_BUFFER == 0xC008_4634);

    #[cfg(target_pointer_width = "32")]
    const _: () = assert!(MXCFB_GET_WORK_BUFFER == 0xC004_4634);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_epdc_ioctl_has_the_number_its_payload_implies() {
        assert_eq!(MXCFB_SET_WAVEFORM_MODES, 0x4018_462B);
        assert_eq!(MXCFB_SET_TEMPERATURE, 0x4004_462C);
        assert_eq!(MXCFB_SET_AUTO_UPDATE_MODE, 0x4004_462D);
        assert_eq!(MXCFB_SEND_UPDATE, 0x4048_462E);
        assert_eq!(MXCFB_WAIT_FOR_UPDATE_COMPLETE, 0xC008_462F);
        assert_eq!(MXCFB_SET_PWRDOWN_DELAY, 0x4004_4630);
        assert_eq!(MXCFB_GET_PWRDOWN_DELAY, 0x8004_4631);
        assert_eq!(MXCFB_SET_UPDATE_SCHEME, 0x4004_4632);
        assert_eq!(MXCFB_DISABLE_EPDC_ACCESS, 0x0000_4635);
        assert_eq!(MXCFB_ENABLE_EPDC_ACCESS, 0x0000_4636);
    }

    #[test]
    fn a_struct_of_a_different_size_gets_a_different_number() {
        // The 64-byte layout that predates `dither_mode` and `quant_bit`
        // — Kobo before Mark 7 — is a different ioctl to the kernel, so
        // sending this crate's struct to one of those is refused rather
        // than misread. The same-size Kindle layout is the case this
        // cannot protect against; see the module docs.
        assert_eq!(iow::<[u8; 64]>(b'F', 0x2E), 0x4040_462E);
        assert_ne!(iow::<[u8; 64]>(b'F', 0x2E), MXCFB_SEND_UPDATE);
        assert_eq!(iow::<[u8; 72]>(b'F', 0x2E), MXCFB_SEND_UPDATE);
    }

    #[test]
    fn a_zeroed_update_is_a_partial_passthrough_with_no_marker() {
        // What `Default` hands a backend that fills fields in: nothing it
        // forgot to set can accidentally ask for a flash, a dither or a
        // wait.
        let u = mxcfb_update_data::default();
        assert_eq!(u.update_mode, UPDATE_MODE_PARTIAL);
        assert_eq!(u.dither_mode, EPDC_FLAG_USE_DITHERING_PASSTHROUGH);
        assert_eq!(u.update_marker, 0);
        assert_eq!(u.flags, 0);
    }
}
