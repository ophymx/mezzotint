//! A [`Panel`] over a plain Linux framebuffer.
//!
//! This backend has no electrophoretic display controller behind it, so it
//! cannot honour an [`UpdateClass`](crate::UpdateClass): there is no waveform to choose and no
//! update to wait for. Pixels written to the mapping are simply on screen.
//! That is the whole point of it.
//!
//! It is here for two reasons. It gives the panel seam a second
//! implementor, and it can run *today*, on a virtual machine or a
//! Raspberry Pi, which no e-ink backend can. `mxcfb` is not a generic
//! e-ink API but NXP's driver for one SoC family; its struct layouts and
//! waveform constants differ per vendor and per model, and the quirks that
//! matter were all found by someone holding the device. Writing that layer
//! without hardware would produce code that runs, which is not the same as
//! code that is right.
//!
//! So everything above [`Panel`] gets exercised end to end here —
//! quantization, damage, and [`PanelDriver`](crate::PanelDriver)'s
//! collision and cleanup rules — against a screen that actually shows the
//! result. What is left for a device is only the part that genuinely needs
//! one.
//!
//! It is also, on its own terms, the crate's honest non-e-ink story: a
//! correct, minimal Rust fbdev output layer is an underserved thing to
//! want.
//!
//! # Which side reduces
//!
//! This backend asks the *consumer* to reduce, through
//! [`PanelInfo::format`] — `Grey { levels: 2, dither: true }` on a mono
//! device, [`PixelFormat::Rgba`] otherwise. It does not act on
//! [`Update::dithering_within`](crate::Update::dithering_within), and the
//! reason is structural rather than an omission: on a framebuffer the
//! pixels reach the screen during [`blit`](Panel::blit), and the
//! diffusion regions arrive one call later at [`submit`](Panel::submit).
//! Only a backend that stages — which is every EPDC, and none of them are
//! written yet — can do the reduction at the point where the class is
//! known. Both channels exist in the contract because it takes hardware to
//! show that the second retires the first.
//!
//! [`encode::quantize_for`](crate::encode::quantize_for) is what a
//! consumer runs to satisfy the request.
//!
//! # Testing without a device
//!
//! A framebuffer's pixel format is decided by hardware, so unit tests can
//! only assert bitfield constants someone typed. QEMU supplies real ones:
//! a host kernel, an initramfs holding `busybox` and the examples, and
//! `vga=` to pick a mode.
//!
//! `scripts/fbdev-vm.sh [vga-mode]` does the whole thing: builds the
//! examples, assembles the initramfs, boots, checks what the guest
//! reported, powers off, and fails the shell if anything went wrong —
//! including when the kernel produced no framebuffer at all, which is a
//! mode that tested nothing rather than a mode that passed.
//!
//! ```text
//! scripts/fbdev-vm.sh 0x314
//! ```
//!
//! `vga=0x314` is 800x600 RGB565, `0x317` is 1024x768 RGB565, `0x315`
//! lands on 24bpp — worth running, since three-byte pixels are the awkward
//! case for byte order — and `0x303` is an 8bpp palette, which this
//! backend refuses. That last one needs a host kernel with
//! `CONFIG_FB_VESA`: a kernel that binds `simpledrm` instead will not
//! drive a palette mode at all, so there is no device to refuse and the
//! harness says so rather than counting it as a refusal. Nothing in it
//! needs root, and it never touches the host's own framebuffer.
//!
//! That is how the palette rejection and the RGB565 packing were checked
//! against a kernel rather than against an assumption.

pub(crate) mod format;

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::time::Duration;

use crate::{
    Error, Panel, PanelInfo, PanelRect, PixelFormat, Result, Source, Update, UpdateToken, Wait,
};

use format::{Bitfield, Encoding, Layout};

// linux/fb.h. Plain constants rather than _IOR-derived: the framebuffer
// ioctls predate the encoding scheme and are literal numbers.
const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

// `fb_fix_screeninfo.visual`. Only the direct ones say that a pixel's bits
// *are* its colour; the pseudocolour visuals say a pixel is an index into
// a palette, which is a different thing entirely and reads as garbage if
// you treat the bitfields as channel placement.
const FB_VISUAL_MONO01: u32 = 0;
const FB_VISUAL_MONO10: u32 = 1;
const FB_VISUAL_TRUECOLOR: u32 = 2;
const FB_VISUAL_PSEUDOCOLOR: u32 = 3;
const FB_VISUAL_DIRECTCOLOR: u32 = 4;
const FB_VISUAL_STATIC_PSEUDOCOLOR: u32 = 5;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct FbFixScreeninfo {
    id: [u8; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    kind: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

impl Default for FbFixScreeninfo {
    fn default() -> Self {
        // Safety: every field is a plain integer or an array of them.
        unsafe { std::mem::zeroed() }
    }
}

/// The structs above are transcriptions of `linux/fb.h`, and a
/// transcription that drifts does not fail — it reads plausible garbage
/// out of the wrong offsets. `--example probe` catches that on whatever
/// machine runs it; these catch it on every target the crate is ever
/// compiled for, including ones nobody has.
///
/// `fb_var_screeninfo` is all `__u32`, so its layout is the same
/// everywhere. `fb_fix_screeninfo` embeds two `unsigned long`, which is
/// eight bytes on a 64-bit target and four on a 32-bit one — and every
/// e-ink device is 32-bit or 64-bit ARM, so both arms of this are real.
mod layout_matches_the_kernel {
    use super::{FbFixScreeninfo, FbVarScreeninfo};
    use std::mem::{offset_of, size_of};

    const _: () = {
        assert!(offset_of!(FbVarScreeninfo, xres) == 0);
        assert!(offset_of!(FbVarScreeninfo, yres) == 4);
        assert!(offset_of!(FbVarScreeninfo, xres_virtual) == 8);
        assert!(offset_of!(FbVarScreeninfo, yres_virtual) == 12);
        assert!(offset_of!(FbVarScreeninfo, bits_per_pixel) == 24);
        assert!(offset_of!(FbVarScreeninfo, grayscale) == 28);
        assert!(offset_of!(FbVarScreeninfo, red) == 32);
        assert!(offset_of!(FbVarScreeninfo, green) == 44);
        assert!(offset_of!(FbVarScreeninfo, blue) == 56);
        assert!(offset_of!(FbVarScreeninfo, transp) == 68);
        assert!(offset_of!(FbVarScreeninfo, rotate) == 136);
        assert!(size_of::<FbVarScreeninfo>() == 160);
    };

    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(offset_of!(FbFixScreeninfo, smem_start) == 16);
        assert!(offset_of!(FbFixScreeninfo, smem_len) == 24);
        assert!(offset_of!(FbFixScreeninfo, visual) == 36);
        assert!(offset_of!(FbFixScreeninfo, line_length) == 48);
        assert!(offset_of!(FbFixScreeninfo, mmio_start) == 56);
        assert!(size_of::<FbFixScreeninfo>() == 80);
    };

    #[cfg(target_pointer_width = "32")]
    const _: () = {
        assert!(offset_of!(FbFixScreeninfo, smem_start) == 16);
        assert!(offset_of!(FbFixScreeninfo, smem_len) == 20);
        assert!(offset_of!(FbFixScreeninfo, visual) == 32);
        assert!(offset_of!(FbFixScreeninfo, line_length) == 44);
        assert!(offset_of!(FbFixScreeninfo, mmio_start) == 48);
        assert!(size_of::<FbFixScreeninfo>() == 68);
    };
}

fn errno(doing: &'static str) -> Error {
    Error::Io {
        doing,
        source: std::io::Error::last_os_error(),
    }
}

/// A framebuffer device, mapped and ready to draw into.
#[derive(Debug)]
pub struct FbdevPanel {
    // Held open for the lifetime of the mapping.
    file: File,
    /// `fb_fix_screeninfo.id`: what the driver calls itself, NUL-padded.
    driver: [u8; 16],
    map: *mut u8,
    map_len: usize,
    layout: Layout,
    width: u32,
    height: u32,
    format: PixelFormat,
    next_token: u32,
}

// Safety: the mapping is owned exclusively by this struct, which hands it
// out only through `&mut self`. Nothing else holds the pointer.
unsafe impl Send for FbdevPanel {}

impl FbdevPanel {
    /// Open the default framebuffer, `/dev/fb0`.
    pub fn open() -> Result<Self> {
        FbdevPanel::open_path("/dev/fb0")
    }

    /// Open a specific framebuffer device.
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|source| Error::Io {
                doing: "open framebuffer",
                source,
            })?;
        let fd = file.as_raw_fd();

        let mut var = FbVarScreeninfo::default();
        let mut fix = FbFixScreeninfo::default();
        // Safety: both ioctls fill a caller-owned struct of the size the
        // kernel expects, and the fd is a framebuffer we just opened.
        unsafe {
            if libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var) < 0 {
                return Err(errno("FBIOGET_VSCREENINFO"));
            }
            if libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix) < 0 {
                return Err(errno("FBIOGET_FSCREENINFO"));
            }
        }

        // 1 is the sub-byte case a bare SPI panel presents; the rest are
        // whole bytes. Nothing between them: 2 and 4 bits per pixel exist
        // on shallow grey e-ink, but the kernel has no field that says
        // which end of the byte their pixels start at, so packing one
        // would be a guess.
        let sub_byte = var.bits_per_pixel == 1;
        if !sub_byte && (var.bits_per_pixel % 8 != 0 || var.bits_per_pixel > 32) {
            return Err(Error::UnsupportedPixels {
                bits_per_pixel: var.bits_per_pixel,
                visual: fix.visual,
                reason: "depth is neither 1 bit nor a whole number of bytes up to four",
            });
        }
        if var.xres == 0 || var.yres == 0 {
            return Err(Error::UnsupportedGeometry {
                width: var.xres,
                height: var.yres,
            });
        }

        let line_length = if fix.line_length > 0 {
            fix.line_length as usize
        } else {
            // Round up, not down: a 1bpp row of 200 pixels is 25 bytes,
            // and integer division by 8 would have made it 0.
            (var.xres as usize * var.bits_per_pixel as usize).div_ceil(8)
        };
        // `smem_len` is what is actually mappable; the visible geometry can
        // be smaller than the virtual one, and a pan offset lives in the
        // difference.
        let map_len = if fix.smem_len > 0 {
            fix.smem_len as usize
        } else {
            line_length * var.yres_virtual.max(var.yres) as usize
        };

        // Classify from what the kernel states rather than from the shape
        // of the bitfields. An 8bpp palette framebuffer reports all three
        // channels at offset 0 with length 8, which is not a layout at all
        // — packing to it ORs the channels together and every colour comes
        // out the same. Guessing produced exactly that; `visual` says so.
        let encoding = if sub_byte {
            // Depth decides before `grayscale` does. A mono framebuffer
            // may well set `grayscale`, and `Encoding::Grey` writes a
            // whole byte per pixel — taking that branch at 1bpp would
            // scribble eight pixels' worth of memory for every one.
            match fix.visual {
                FB_VISUAL_MONO10 => Encoding::Mono { one_is_white: true },
                FB_VISUAL_MONO01 => Encoding::Mono {
                    one_is_white: false,
                },
                // Every 1bpp framebuffer is one or the other. If a driver
                // says otherwise, the polarity is unknown, and a guess
                // shows up as a negative image rather than as an error.
                other => {
                    return Err(Error::UnsupportedPixels {
                        bits_per_pixel: 1,
                        visual: other,
                        reason: "one bit per pixel but not monochrome, so which bit \
                                 means white is unknown",
                    });
                }
            }
        } else if var.grayscale != 0 {
            Encoding::Grey
        } else {
            match fix.visual {
                FB_VISUAL_TRUECOLOR | FB_VISUAL_DIRECTCOLOR => Encoding::Channels {
                    red: Bitfield {
                        offset: var.red.offset,
                        length: var.red.length,
                    },
                    green: Bitfield {
                        offset: var.green.offset,
                        length: var.green.length,
                    },
                    blue: Bitfield {
                        offset: var.blue.offset,
                        length: var.blue.length,
                    },
                },
                other => {
                    let reason = match other {
                        // Mono is handled above, at the only depth it
                        // comes in; reaching here means a driver claimed
                        // monochrome pixels a byte or more wide.
                        FB_VISUAL_MONO01 | FB_VISUAL_MONO10 => {
                            "monochrome visual at more than one bit per pixel"
                        }
                        FB_VISUAL_PSEUDOCOLOR | FB_VISUAL_STATIC_PSEUDOCOLOR => {
                            "palette framebuffer: its pixels are indices, not colours, and \
                             supporting it means installing a colormap"
                        }
                        _ => "unrecognized visual",
                    };
                    return Err(Error::UnsupportedPixels {
                        bits_per_pixel: var.bits_per_pixel,
                        visual: other,
                        reason,
                    });
                }
            }
        };

        // Safety: mapping a framebuffer we hold open, with a length the
        // kernel just reported for it. Deliberately after the format
        // checks: a device this backend is going to refuse should not be
        // mapped on the way to refusing it.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                map_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(errno("mmap framebuffer"));
        }

        Ok(FbdevPanel {
            file,
            driver: fix.id,
            map: map.cast::<u8>(),
            map_len,
            layout: Layout {
                bits_per_pixel: var.bits_per_pixel as usize,
                width: var.xres,
                line_length,
                encoding,
            },
            width: var.xres,
            height: var.yres,
            // Full colour by default even on a greyscale framebuffer: it
            // can show 256 levels, so asking the consumer to quantize to
            // e-ink's sixteen would throw away quality for nothing. Use
            // `with_format` to emulate a shallower panel deliberately.
            //
            // A mono framebuffer is the exception, and the only place a
            // non-default is honest: it has exactly two levels, so the
            // blit's threshold is going to reduce to them whatever it is
            // handed. Asking for it up front means the reduction happens
            // where the error can be diffused instead of one pixel at a
            // time — which at two levels is the difference between a
            // photograph and a silhouette.
            //
            // Asking for dithering does not mean dithering everywhere:
            // `encode::quantize_for` diffuses only inside the regions the
            // consumer names, and quantizes the rest plainly. That is what
            // makes this request safe on a page that is mostly text.
            format: match encoding {
                Encoding::Mono { .. } => PixelFormat::Grey {
                    levels: 2,
                    dither: true,
                },
                _ => PixelFormat::Rgba,
            },
            next_token: 1,
        })
    }

    /// Report a different [`PixelFormat`] than the device's own, so the
    /// consumer quantizes as if for a shallower panel.
    ///
    /// The point is being able to see what a 16-level e-ink page will look
    /// like without owning one.
    pub fn with_format(mut self, format: PixelFormat) -> Self {
        self.format = format;
        self
    }

    /// A description of the device, for a shell that wants to say what it
    /// found.
    pub fn describe(&self) -> String {
        let depth = self.layout.bits_per_pixel;
        let encoding = match self.layout.encoding {
            Encoding::Grey => "grey".to_string(),
            // Say which way round it is: a mono panel wired the other way
            // shows a flawless negative, and the string is the only place
            // that becomes visible before the glass does.
            Encoding::Mono { one_is_white } => {
                format!("mono {}=white", u8::from(one_is_white))
            }
            // Offsets as well as widths: a wrong `#[repr(C)]` shows up
            // here as nonsense, where a swapped channel order would
            // otherwise only be visible as wrong colours on the glass.
            Encoding::Channels { red, green, blue } => format!(
                "R{}@{} G{}@{} B{}@{}",
                red.length, red.offset, green.length, green.offset, blue.length, blue.offset
            ),
        };
        format!(
            "{}x{} {depth}bpp {encoding}, stride {}, {} bytes mapped",
            self.width, self.height, self.layout.line_length, self.map_len
        )
    }

    /// The device's channel placement as `(offset, length)` for red,
    /// green and blue, or `None` on a greyscale or mono framebuffer.
    ///
    /// Exposed for bring-up: a self-test can work out what a colour
    /// *should* pack to from the device's own numbers, instead of from a
    /// constant someone typed while guessing at the hardware.
    pub fn channels(&self) -> Option<[(u32, u32); 3]> {
        match self.layout.encoding {
            Encoding::Grey | Encoding::Mono { .. } => None,
            Encoding::Channels { red, green, blue } => Some([
                (red.offset, red.length),
                (green.offset, green.length),
                (blue.offset, blue.length),
            ]),
        }
    }

    /// Read one pixel back out of the framebuffer as the device spells it.
    ///
    /// A diagnostic, not a drawing primitive: it is how a self-test
    /// confirms that what reached the hardware is what was meant, on a
    /// panel nobody has tried before.
    pub fn read_pixel(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let Some(bpp) = self.layout.bytes_per_pixel() else {
            // Sub-byte: the pixel is a bit inside a byte, so read the byte
            // and return the bit as 0 or 1 — the same value `pack`
            // produced for it, which is what a self-test compares against.
            let at = y as usize * self.layout.line_length + x as usize / 8;
            if at >= self.map_len {
                return None;
            }
            // Safety: `at` is inside the mapping, which lives as long as
            // `&self`.
            let byte = unsafe { *self.map.add(at) };
            return Some(u32::from((byte >> (7 - x % 8)) & 1));
        };
        let at = y as usize * self.layout.line_length + x as usize * bpp;
        if at + bpp > self.map_len {
            return None;
        }
        // Safety: `at + bpp` is inside the mapping, which lives as long as
        // `&self`.
        let bytes = unsafe { std::slice::from_raw_parts(self.map.add(at), bpp) };
        Some(format::read_native(bytes))
    }

    /// The driver's name from `fb_fix_screeninfo.id`, as text.
    ///
    /// What the kernel calls the thing behind `/dev/fb0` — `hwtcon`,
    /// `mxc_epdc_fb`, `vesafb` — which is how a bring-up tool decides
    /// which backend to try, and the first fact to print on a device
    /// nobody has seen.
    pub fn driver_id(&self) -> &str {
        crate::error::nul_terminated(&self.driver)
    }

    /// The driver's name as the kernel spelled it.
    #[cfg_attr(not(feature = "hwtcon"), allow(dead_code))]
    pub(crate) fn driver_id_bytes(&self) -> [u8; 16] {
        self.driver
    }

    /// The descriptor, for a backend that layers ioctls over this one.
    #[cfg_attr(not(feature = "hwtcon"), allow(dead_code))]
    pub(crate) fn raw_fd(&self) -> std::os::unix::io::RawFd {
        self.file.as_raw_fd()
    }

    #[cfg_attr(not(feature = "hwtcon"), allow(dead_code))]
    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    pub(crate) fn mapping(&mut self) -> &mut [u8] {
        // Safety: `map` came from a successful mmap of `map_len` bytes and
        // is unmapped only in `Drop`, so it is valid for the whole life of
        // this borrow, and `&mut self` makes the slice exclusive.
        unsafe { std::slice::from_raw_parts_mut(self.map, self.map_len) }
    }
}

impl Drop for FbdevPanel {
    fn drop(&mut self) {
        // Safety: unmapping exactly the region this struct mapped, once.
        unsafe {
            libc::munmap(self.map.cast::<libc::c_void>(), self.map_len);
        }
    }
}

impl Panel for FbdevPanel {
    fn info(&self) -> PanelInfo {
        PanelInfo::new(self.width, self.height, self.format)
    }

    fn blit(&mut self, src: Source<'_>, rect: PanelRect) -> Result<()> {
        src.require(rect)?;
        let layout = self.layout;
        format::blit_into(self.mapping(), &layout, src, rect);
        Ok(())
    }

    /// No-op beyond handing back a token.
    ///
    /// A framebuffer has no update to request: the pixels went on screen
    /// during [`Panel::blit`]. The [`UpdateClass`](crate::UpdateClass) is accepted and
    /// ignored, which is honest — this is the backend that shows what the
    /// seam looks like when the panel underneath cannot do anything
    /// clever.
    fn submit(&mut self, _rect: PanelRect, _update: Update<'_>) -> Result<UpdateToken> {
        let token = UpdateToken::new(self.next_token);
        // Never zero: some controllers use 0 as "no update", and a token
        // that collides with that reads as already-complete.
        self.next_token = self.next_token.wrapping_add(1).max(1);
        Ok(token)
    }

    /// Returns immediately: nothing was ever in flight.
    fn wait(&mut self, _token: UpdateToken, _timeout: Option<Duration>) -> Result<Wait> {
        Ok(Wait::Complete)
    }
}
