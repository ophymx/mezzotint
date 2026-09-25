//! The i.MX EPDC ioctl vocabulary — and, deliberately, not yet a
//! [`Panel`](crate::Panel).
//!
//! `mxcfb` is NXP's framebuffer driver for the i.MX SoC family, and its
//! electrophoretic display controller half is the interface every
//! i.MX-based e-reader exposes: Kobo up to the Allwinner models, every
//! Kindle before the MediaTek ones, the first reMarkable. An update is a
//! struct handed to one ioctl, and completion is a marker handed to
//! another. That surface is what [`uapi`] transcribes.
//!
//! # Why the vocabulary and not the backend
//!
//! `CONTRIBUTING.md` gates an EPDC backend on hardware, and the gate is
//! respected here rather than skirted. Which waveform constant means
//! "fast and monochrome" on which model, and what a full-mode `GC16` does
//! to a page that was half-way through an `A2` drag, are only visible on
//! glass; a backend written from headers alone would run without being
//! right. But the struct layouts and ioctl numbers are facts a kernel
//! header states outright, and a transcription of them is checkable on a
//! build machine: `const` assertions on every offset and size, on both
//! the 32-bit and the 64-bit arm, because every e-ink device is one or the
//! other. So this much is written, and is what the backend will be
//! written *against* when there is a device to hold.
//!
//! Nothing in this module opens a file or issues an ioctl. It needs
//! neither `std` nor `libc`, and its tests run on the host.
//!
//! # The number is derived from the struct, on purpose
//!
//! An ioctl number encodes the size of its payload, so `MXCFB_SEND_UPDATE`
//! is not one constant but one per struct layout — and there are several
//! in the wild, because every vendor forked the header. The shapes that
//! exist, stated as facts so a future backend does not learn them the hard
//! way:
//!
//! - **64 bytes**: the original layout, with no `dither_mode` or
//!   `quant_bit`. The stock Freescale 2.6.35 and 3.0.35 BSPs, and the
//!   i.MX50 Kindles.
//! - **68 bytes, on 32-bit**: that same layout as NTX shipped it in every
//!   Kobo before Mark 7, with a kernel pointer as the first field of the
//!   alt-buffer data. `MXCFB_SEND_UPDATE` is `0x4044462E` there, and the
//!   wait takes a bare `__u32` marker rather than the marker-data struct.
//! - **72 bytes**: the layout in [`uapi`]. NXP's own header from the 4.1
//!   branch onward, unchanged through 6.6; Kobo Mark 7 and later; the
//!   reMarkable 1.
//! - **72 bytes, again**: the i.MX6 Kindles reach the same size by a
//!   different route — two histogram-waveform fields inserted *before*
//!   `temp`, and no dither fields at all. Same ioctl number, `temp` at a
//!   different offset. This is the case the size cannot catch.
//! - **80 and 88 bytes**: later Kindles, this layout plus trailing
//!   Lab126 fields.
//!
//! The kernel switches on the whole number, so a payload of the wrong
//! *size* is refused with `EINVAL` — loud, and easy to diagnose. A payload
//! of the right size and the wrong *layout* is accepted, and the
//! controller reads a waveform mode out of the temperature slot. Only the
//! device table can tell those apart, which is why the ioctl surface, the
//! waveform table and the device table are three separate things. The
//! table is [`devices`](crate::devices); the waveform table is the part
//! that waits for hardware.
//!
//! [`uapi`] therefore never spells an ioctl number as a literal. Each is
//! computed from the struct it carries by the kernel's own `_IOW`
//! encoding, so the two cannot drift apart, and the two numbers that other
//! implementations publish independently — `0x4048462E` for the update
//! and `0xC008462F` for the wait — are asserted against as a cross-check
//! on the encoding itself.
//!
//! # What the header does not say
//!
//! The UAPI names only three waveform modes: the two REAGL variants and
//! `AUTO`. `INIT`, `DU`, `GC16`, `A2` and the rest are numbered by each
//! vendor's driver and differ between them, which is the waveform table,
//! and it arrives with the hardware.
//!
//! # Provenance
//!
//! Transcribed from `include/uapi/linux/mxcfb.h` in NXP's `linux-imx`
//! tree. The file is byte-identical on the `imx_4.1.15_1.0.0_ga`,
//! `imx_4.9.88_2.0.0_ga` and `lf-6.6.y` branches, checked on 2026-09-24,
//! so one citation covers the whole i.MX EPDC era. Kernel UAPI headers are
//! GPL-2.0 with the Linux syscall note, which is what makes user-space
//! transcription of them unencumbered; nothing here is taken from any
//! other project's copy of them.

pub mod uapi;
