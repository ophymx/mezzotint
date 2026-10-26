//! `include/linux/hwtcon_ioctl_cmd.h`, transcribed.
//!
//! Names are the header's own, C spelling and all, so that a reader can
//! diff this against the source line by line; see the
//! [module docs](super) for the citation. The ioctl numbers are derived
//! from the structs by the kernel's own `_IOW` encoding, for the reasons
//! [`mxcfb`](crate::mxcfb) gives, and the ones measured against a running
//! kernel are asserted as a cross-check.
//!
//! ```
//! use mezzotint::hwtcon::uapi::*;
//!
//! let update = hwtcon_update_data {
//!     update_region: hwtcon_rect { top: 40, left: 0, width: 600, height: 20 },
//!     waveform_mode: WAVEFORM_MODE_GL16,
//!     update_mode: UPDATE_MODE_PARTIAL,
//!     update_marker: 1,
//!     flags: 0,
//!     dither_mode: 0,
//! };
//! assert_eq!(update.update_region.width, 600);
//! assert_eq!(HWTCON_SEND_UPDATE, 0x4024_462E);
//! ```

#![allow(non_camel_case_types)]

use core::mem::size_of;

use crate::ioctl::{ioc, ior, iow, iowr, IOC_READ};

// ---------------------------------------------------------------------
// include/linux/hwtcon_ioctl_cmd.h
// ---------------------------------------------------------------------

/// A region of the framebuffer, in the controller's own order: top before
/// left.
///
/// In *framebuffer* coordinates, not the panel's native ones: the driver
/// rotates it by the framebuffer's `rotate` itself
/// (`hwtcon_core_rotate_region`), so a backend hands over the rect it was
/// given. Not [`PanelRect`](crate::PanelRect), and not convertible by
/// accident; a backend does the field reordering in one place.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct hwtcon_rect {
    /// First row.
    pub top: u32,
    /// First column.
    pub left: u32,
    /// Columns.
    pub width: u32,
    /// Rows.
    pub height: u32,
}

/// `flags`: run the hardware ditherer, with the algorithm in
/// `dither_mode`.
pub const HWTCON_FLAG_USE_DITHERING: u32 = 0x1;
/// `flags`: with the `A2` waveform, force every pixel to one of two
/// levels before driving.
pub const HWTCON_FLAG_FORCE_A2_OUTPUT: u32 = 0x10;
/// `flags`: with [`HWTCON_FLAG_FORCE_A2_OUTPUT`], the forced level is
/// white.
pub const HWTCON_FLAG_FORCE_A2_OUTPUT_WHITE: u32 = 0x20;
/// `flags`: with [`HWTCON_FLAG_FORCE_A2_OUTPUT`], the forced level is
/// black.
pub const HWTCON_FLAG_FORCE_A2_OUTPUT_BLACK: u32 = 0x40;
/// `flags`: drive this update as black and white even on a colour panel,
/// skipping the colour-filter-array pass.
pub const HWTCON_FLAG_CFA_SKIP: u32 = 0x0000_8000;
/// `flags`: the bits that select a colour-filter-array mode for this
/// update, one of the `HWTCON_FLAG_CFA_*` values; zero leaves the driver's
/// global setting in force.
pub const HWTCON_FLAG_CFA_FLDS_MASK: u32 = 0x0000_7f00;
/// CFA mode: E Ink's gain algorithm at gain 1, the driver's default.
pub const HWTCON_FLAG_CFA_EINK_NORMAL: u32 = 0x0000_0100;
/// CFA mode: the same value as [`HWTCON_FLAG_CFA_EINK_NORMAL`].
pub const HWTCON_FLAG_CFA_EINK_G1: u32 = 0x0000_0100;
/// CFA mode: E Ink's AIE algorithm, S4.
pub const HWTCON_FLAG_CFA_EINK_AIE_S4: u32 = 0x0000_0200;
/// CFA mode: E Ink's AIE algorithm, S7.
pub const HWTCON_FLAG_CFA_EINK_AIE_S7: u32 = 0x0000_0300;
/// CFA mode: E Ink's AIE algorithm, S9.
pub const HWTCON_FLAG_CFA_EINK_AIE_S9: u32 = 0x0000_0400;
/// CFA mode: E Ink's gain algorithm at gain 0.
pub const HWTCON_FLAG_CFA_EINK_G0: u32 = 0x0000_0500;
/// CFA mode: E Ink's gain algorithm at gain 2.
pub const HWTCON_FLAG_CFA_EINK_G2: u32 = 0x0000_0600;
/// CFA mode: the device maker's own algorithm.
pub const HWTCON_FLAG_CFA_EINK_NTX: u32 = 0x0000_0a00;
/// CFA mode: the device maker's own algorithm, the `SF` variant.
pub const HWTCON_FLAG_CFA_EINK_NTX_SF: u32 = 0x0000_0b00;

/// `HWTCON_SET_TEMPERATURE`: the header's sentinel for "use the sensor".
///
/// Do not send it. On the 4.9 driver the ioctl stores its argument as the
/// panel temperature without looking at it, and the sentinel is only
/// recognised on a separate debug path — so sending this sets the
/// temperature to 1048576 degrees and the waveform lookup clamps to its
/// top zone. Read back from a device, where `HWTCON_GET_TEMPERATURE`
/// returned the sentinel verbatim afterwards — and the driver's own
/// periodic sensor read is compiled out, so nothing corrects it until
/// user space does. The temperature is user space's to set, in degrees.
pub const TEMP_USE_SENSOR: i32 = 0x10_0000;

/// Only pixels that changed are driven.
pub const UPDATE_MODE_PARTIAL: u32 = 0x0;
/// Every pixel in the region is driven, which with a 16-level waveform is
/// the flash.
pub const UPDATE_MODE_FULL: u32 = 0x1;

/// Clearing the panel.
pub const WAVEFORM_MODE_INIT: u32 = 0;
/// Direct update: two levels, fast.
pub const WAVEFORM_MODE_DU: u32 = 1;
/// Sixteen levels, flashing in full mode.
pub const WAVEFORM_MODE_GC16: u32 = 2;
/// Sixteen levels without the flash.
pub const WAVEFORM_MODE_GL16: u32 = 3;
/// REAGL: sixteen levels, no flash, ghost compensation.
pub const WAVEFORM_MODE_GLR16: u32 = 4;
/// The same value as [`WAVEFORM_MODE_GLR16`].
pub const WAVEFORM_MODE_REAGL: u32 = 4;
/// Two levels, the fastest the panel has; for things that are moving.
pub const WAVEFORM_MODE_A2: u32 = 6;
/// Sixteen levels for a dark background: the night-mode counterpart of
/// `GC16`.
pub const WAVEFORM_MODE_GCK16: u32 = 8;
/// The night-mode counterpart of `GL16`.
pub const WAVEFORM_MODE_GLKW16: u32 = 9;
/// Sixteen levels for colour content.
pub const WAVEFORM_MODE_GCC16: u32 = 10;
/// REAGL for colour content.
pub const WAVEFORM_MODE_GLRC16: u32 = 11;
/// Let the driver pick a waveform from the pixels it was given.
pub const WAVEFORM_MODE_AUTO: u32 = 257;

/// `dither_mode`: 8-bit to 4-bit, algorithm `Q`.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y4_Q: i32 = 0x100;
/// `dither_mode`: 8-bit to 2-bit, algorithm `Q`.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y2_Q: i32 = 0x200;
/// `dither_mode`: 8-bit to 1-bit, algorithm `Q`.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y1_Q: i32 = 0x300;
/// `dither_mode`: 4-bit to 2-bit, algorithm `Q`.
pub const HWTCON_FLAG_USE_DITHERING_Y4_Y2_Q: i32 = 0x10200;
/// `dither_mode`: 4-bit to 1-bit, algorithm `Q`.
pub const HWTCON_FLAG_USE_DITHERING_Y4_Y1_Q: i32 = 0x10300;
/// `dither_mode`: 8-bit to 4-bit, algorithm `B`.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y4_B: i32 = 0x101;
/// `dither_mode`: 8-bit to 2-bit, algorithm `B`.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y2_B: i32 = 0x201;
/// `dither_mode`: 8-bit to 1-bit, algorithm `B`.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y1_B: i32 = 0x301;
/// `dither_mode`: 4-bit to 2-bit, algorithm `B`.
pub const HWTCON_FLAG_USE_DITHERING_Y4_Y2_B: i32 = 0x10201;
/// `dither_mode`: 4-bit to 1-bit, algorithm `B`.
pub const HWTCON_FLAG_USE_DITHERING_Y4_Y1_B: i32 = 0x10301;
/// `dither_mode`: 8-bit to 4-bit, algorithm `S` — the one the driver
/// itself defaults to when it dithers on its own account.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y4_S: i32 = 0x102;
/// `dither_mode`: 8-bit to 2-bit, algorithm `S`.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y2_S: i32 = 0x202;
/// `dither_mode`: 8-bit to 1-bit, algorithm `S`.
pub const HWTCON_FLAG_USE_DITHERING_Y8_Y1_S: i32 = 0x302;
/// `dither_mode`: 4-bit to 2-bit, algorithm `S`.
pub const HWTCON_FLAG_USE_DITHERING_Y4_Y2_S: i32 = 0x10202;
/// `dither_mode`: 4-bit to 1-bit, algorithm `S`.
pub const HWTCON_FLAG_USE_DITHERING_Y4_Y1_S: i32 = 0x10302;

/// The payload of [`HWTCON_SET_WAVEFORM_MODES`]: which of the panel's
/// waveform table entries the driver should use for each purpose.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct hwtcon_waveform_modes {
    /// Clearing the panel.
    pub mode_init: i32,
    /// Direct update, black and white.
    pub mode_du: i32,
    /// Sixteen grey levels.
    pub mode_gc16: i32,
    /// Sixteen grey levels without the flash.
    pub mode_gl16: i32,
    /// Two levels, fastest.
    pub mode_a2: i32,
    /// REAGL.
    pub mode_reagl: i32,
}

/// The payload of [`HWTCON_WAIT_FOR_UPDATE_COMPLETE`].
///
/// The driver reads only `update_marker` out of it
/// (`hwtcon_fb_ioctl_wait_for_task_displayed` copies a `u32`), so
/// `collision_test` comes back as it went in. The size is still the
/// struct's, because the ioctl number encodes it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct hwtcon_update_marker_data {
    /// The marker from the update being waited on.
    pub update_marker: u32,
    /// Declared for collision reporting; not written by this driver.
    pub collision_test: u32,
}

/// One update request: the payload of [`HWTCON_SEND_UPDATE`].
///
/// Smaller than any `mxcfb_update_data`: no temperature (that is a
/// separate ioctl here), no quantization depth (the dither mode carries
/// it) and no alternate buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct hwtcon_update_data {
    /// The region to refresh, in framebuffer coordinates.
    pub update_region: hwtcon_rect,
    /// One of the `WAVEFORM_MODE_*` values.
    pub waveform_mode: u32,
    /// [`UPDATE_MODE_PARTIAL`] or [`UPDATE_MODE_FULL`].
    pub update_mode: u32,
    /// The marker the wait ioctls will wait on.
    pub update_marker: u32,
    /// Any of the `HWTCON_FLAG_*` bits.
    pub flags: u32,
    /// One of the `HWTCON_FLAG_USE_DITHERING_*` modes, read when
    /// [`HWTCON_FLAG_USE_DITHERING`] is set.
    pub dither_mode: i32,
}

/// The payload of [`HWTCON_GET_PANEL_INFO`]: what the driver knows about
/// the glass it is driving.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct hwtcon_panel_info {
    /// The waveform file's name, NUL-terminated; it names the panel.
    pub wf_file_name: [u8; 100],
    /// VCOM in millivolts, as read from the power IC.
    pub vcom_value: i32,
    /// Panel temperature in degrees Celsius.
    pub temp: i32,
    /// The waveform table's temperature zone for that temperature.
    pub temp_zone: i32,
}

impl Default for hwtcon_panel_info {
    fn default() -> Self {
        hwtcon_panel_info {
            wf_file_name: [0; 100],
            vcom_value: 0,
            temp: 0,
            temp_zone: 0,
        }
    }
}

impl core::fmt::Debug for hwtcon_panel_info {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("hwtcon_panel_info")
            .field(
                "wf_file_name",
                &crate::error::nul_terminated(&self.wf_file_name),
            )
            .field("vcom_value", &self.vcom_value)
            .field("temp", &self.temp)
            .field("temp_zone", &self.temp_zone)
            .finish()
    }
}

/// The ioctl type every command here carries — the same letter as mxcfb,
/// and most of the same numbers.
pub const HWTCON_IOCTL_MAGIC_NUMBER: u8 = b'F';

/// Switch night mode on or off.
pub const HWTCON_SET_NIGHTMODE: u32 = iow::<i32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x26);
/// Tell the driver which waveform table entries to use for each purpose.
pub const HWTCON_SET_WAVEFORM_MODES: u32 =
    iow::<hwtcon_waveform_modes>(HWTCON_IOCTL_MAGIC_NUMBER, 0x2B);
/// Set the temperature the waveform tables are indexed by, or
/// [`TEMP_USE_SENSOR`].
pub const HWTCON_SET_TEMPERATURE: u32 = iow::<i32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x2C);
/// Accepted and ignored by the driver.
pub const HWTCON_SET_AUTO_UPDATE_MODE: u32 = iow::<u32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x2D);
/// Submit an update.
pub const HWTCON_SEND_UPDATE: u32 = iow::<hwtcon_update_data>(HWTCON_IOCTL_MAGIC_NUMBER, 0x2E);
/// Block until the update with this marker has finished reaching the
/// glass, or the driver's own five-second limit passes.
pub const HWTCON_WAIT_FOR_UPDATE_COMPLETE: u32 =
    iowr::<hwtcon_update_marker_data>(HWTCON_IOCTL_MAGIC_NUMBER, 0x2F);
/// Milliseconds of idleness before the controller powers down.
pub const HWTCON_SET_PWRDOWN_DELAY: u32 = iow::<i32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x30);
/// Read the power-down delay back.
pub const HWTCON_GET_PWRDOWN_DELAY: u32 = ior::<i32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x31);
/// Discard every update until [`HWTCON_SET_RESUME`].
pub const HWTCON_SET_PAUSE: u32 = iow::<u32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x33);
/// Copy the controller's working buffer out. The payload is `unsigned
/// long`, so the number differs between a 32-bit and a 64-bit kernel.
pub const HWTCON_GET_WORK_BUFFER: u32 = iowr::<usize>(HWTCON_IOCTL_MAGIC_NUMBER, 0x34);
/// Whether updates are paused. Shares its number with
/// [`HWTCON_GET_WORK_BUFFER`] in the header; the two differ only in the
/// direction bits, which is what keeps them distinct ioctls.
pub const HWTCON_GET_PAUSE: u32 = iow::<u32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x34);
/// Accept updates again after [`HWTCON_SET_PAUSE`].
pub const HWTCON_SET_RESUME: u32 = iow::<u32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x35);
/// Block until the update with this marker has been handed to the
/// controller — not yet on the glass.
pub const HWTCON_WAIT_FOR_UPDATE_SUBMISSION: u32 = iow::<u32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x37);
/// Read the temperature the waveform tables are currently indexed by.
pub const HWTCON_GET_TEMPERATURE: u32 = ior::<i32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x38);
/// Set the global colour-filter-array mode, in the driver's own numbering
/// (not the `HWTCON_FLAG_CFA_*` values).
pub const HWTCON_SET_CFA_MODE: u32 = iow::<u32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x50);
/// Read the global colour-filter-array mode. Declared `_IOW` in the header
/// although the kernel writes it; the direction bits are the header's.
pub const HWTCON_GET_CFA_MODE: u32 = iow::<u32>(HWTCON_IOCTL_MAGIC_NUMBER, 0x51);
/// Ask the driver what glass it is driving.
///
/// The header writes this as `_IOR('F', 0x130, struct hwtcon_panel_info)`
/// with a number that does not fit the eight bits the encoding gives it.
/// The C macro does not mask, so the ninth bit lands in the type field
/// and the kernel matches on that value — type `'G'`, number `0x30` — and
/// nothing else. The constant reproduces the overflow rather than the
/// intent, because the overflow is what the kernel compares against; it
/// was confirmed to answer on a device.
pub const HWTCON_GET_PANEL_INFO: u32 = ioc(
    IOC_READ,
    HWTCON_IOCTL_MAGIC_NUMBER,
    0x130,
    size_of::<hwtcon_panel_info>(),
);

/// Every field in the header is `__u32`, `int` or `char[]`, so these
/// layouts are the same on both arms; the assertions are unconditional so
/// the armv7 and aarch64 checks in CI each evaluate them.
///
/// The numbers pinned here were read back from a running 4.9 kernel on a
/// MediaTek device, which makes them the independent check on the
/// encoding — and on the overflowed one in particular.
mod layout_matches_the_kernel {
    use super::*;
    use core::mem::offset_of;

    const _: () = {
        assert!(size_of::<hwtcon_rect>() == 16);
        assert!(offset_of!(hwtcon_rect, top) == 0);
        assert!(offset_of!(hwtcon_rect, left) == 4);
        assert!(offset_of!(hwtcon_rect, width) == 8);
        assert!(offset_of!(hwtcon_rect, height) == 12);

        assert!(size_of::<hwtcon_update_data>() == 36);
        assert!(offset_of!(hwtcon_update_data, update_region) == 0);
        assert!(offset_of!(hwtcon_update_data, waveform_mode) == 16);
        assert!(offset_of!(hwtcon_update_data, update_mode) == 20);
        assert!(offset_of!(hwtcon_update_data, update_marker) == 24);
        assert!(offset_of!(hwtcon_update_data, flags) == 28);
        assert!(offset_of!(hwtcon_update_data, dither_mode) == 32);

        assert!(size_of::<hwtcon_update_marker_data>() == 8);
        assert!(offset_of!(hwtcon_update_marker_data, collision_test) == 4);

        assert!(size_of::<hwtcon_waveform_modes>() == 24);
        assert!(offset_of!(hwtcon_waveform_modes, mode_reagl) == 20);

        assert!(size_of::<hwtcon_panel_info>() == 112);
        assert!(offset_of!(hwtcon_panel_info, vcom_value) == 100);
        assert!(offset_of!(hwtcon_panel_info, temp) == 104);
        assert!(offset_of!(hwtcon_panel_info, temp_zone) == 108);

        assert!(HWTCON_SEND_UPDATE == 0x4024_462E);
        assert!(HWTCON_WAIT_FOR_UPDATE_COMPLETE == 0xC008_462F);
        assert!(HWTCON_WAIT_FOR_UPDATE_SUBMISSION == 0x4004_4637);
        assert!(HWTCON_GET_TEMPERATURE == 0x8004_4638);
        assert!(HWTCON_GET_CFA_MODE == 0x4004_4651);
        assert!(HWTCON_GET_PANEL_INFO == 0x8070_4730);
    };

    #[cfg(target_pointer_width = "64")]
    const _: () = assert!(HWTCON_GET_WORK_BUFFER == 0xC008_4634);

    #[cfg(target_pointer_width = "32")]
    const _: () = assert!(HWTCON_GET_WORK_BUFFER == 0xC004_4634);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hwtcon_ioctl_has_the_number_its_payload_implies() {
        assert_eq!(HWTCON_SET_NIGHTMODE, 0x4004_4626);
        assert_eq!(HWTCON_SET_WAVEFORM_MODES, 0x4018_462B);
        assert_eq!(HWTCON_SET_TEMPERATURE, 0x4004_462C);
        assert_eq!(HWTCON_SET_AUTO_UPDATE_MODE, 0x4004_462D);
        assert_eq!(HWTCON_SEND_UPDATE, 0x4024_462E);
        assert_eq!(HWTCON_WAIT_FOR_UPDATE_COMPLETE, 0xC008_462F);
        assert_eq!(HWTCON_SET_PWRDOWN_DELAY, 0x4004_4630);
        assert_eq!(HWTCON_GET_PWRDOWN_DELAY, 0x8004_4631);
        assert_eq!(HWTCON_SET_PAUSE, 0x4004_4633);
        assert_eq!(HWTCON_GET_PAUSE, 0x4004_4634);
        assert_eq!(HWTCON_SET_RESUME, 0x4004_4635);
        assert_eq!(HWTCON_WAIT_FOR_UPDATE_SUBMISSION, 0x4004_4637);
        assert_eq!(HWTCON_GET_TEMPERATURE, 0x8004_4638);
        assert_eq!(HWTCON_SET_CFA_MODE, 0x4004_4650);
        assert_eq!(HWTCON_GET_CFA_MODE, 0x4004_4651);
    }

    #[test]
    fn the_update_is_a_different_ioctl_from_mxcfbs() {
        // Same letter, same number, smaller struct: a 36-byte payload
        // handed to an i.MX kernel, or a 72-byte one handed to this
        // driver, is refused as an unknown command rather than misread.
        assert_eq!(iow::<[u8; 72]>(b'F', 0x2E), 0x4048_462E);
        assert_ne!(HWTCON_SEND_UPDATE, iow::<[u8; 72]>(b'F', 0x2E));
    }

    #[test]
    fn get_pause_and_get_work_buffer_share_a_number_but_not_a_command() {
        assert_ne!(HWTCON_GET_PAUSE, HWTCON_GET_WORK_BUFFER);
        assert_eq!(HWTCON_GET_PAUSE & 0xFF, HWTCON_GET_WORK_BUFFER & 0xFF);
    }

    #[test]
    fn a_zeroed_update_is_a_partial_init_with_no_marker_and_no_flags() {
        let u = hwtcon_update_data::default();
        assert_eq!(u.update_mode, UPDATE_MODE_PARTIAL);
        assert_eq!(u.waveform_mode, WAVEFORM_MODE_INIT);
        assert_eq!(u.update_marker, 0);
        assert_eq!(u.flags, 0);
        assert_eq!(u.dither_mode, 0);
    }

    #[test]
    fn panel_info_debug_shows_the_name_as_text() {
        let mut info = hwtcon_panel_info::default();
        info.wf_file_name[..5].copy_from_slice(b"EC060");
        info.vcom_value = -2920;
        let s = format!("{info:?}");
        assert!(s.contains("\"EC060\""), "{s}");
        assert!(s.contains("-2920"), "{s}");
    }
}
