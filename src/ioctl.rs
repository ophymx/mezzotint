//! The kernel's ioctl number encoding, so a transcribed header can derive
//! its numbers from its structs instead of writing them down.
//!
//! `include/uapi/asm-generic/ioctl.h`: two bits of direction, fourteen of
//! payload size, eight of type, eight of number. Every architecture this
//! crate can run on — arm, arm64, x86 — takes the generic encoding.
//!
//! An ioctl number encodes the size of its payload, so a struct of the
//! wrong size produces a *different number* and the kernel refuses it with
//! `EINVAL` rather than reading garbage out of it. Deriving the number
//! from the struct keeps the two from drifting apart; the handful of
//! numbers other implementations publish independently are asserted
//! against in each transcription as a cross-check on this encoding.

use core::mem::size_of;

const IOC_NRBITS: u32 = 8;
const IOC_TYPEBITS: u32 = 8;
const IOC_SIZEBITS: u32 = 14;

const IOC_NRSHIFT: u32 = 0;
const IOC_TYPESHIFT: u32 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u32 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u32 = IOC_SIZESHIFT + IOC_SIZEBITS;

/// `_IOC_NONE`.
#[cfg_attr(not(feature = "mxcfb"), allow(dead_code))]
pub const IOC_NONE: u32 = 0;
/// `_IOC_WRITE`: user space hands the kernel the payload.
pub const IOC_WRITE: u32 = 1;
/// `_IOC_READ`: the kernel writes the payload back.
pub const IOC_READ: u32 = 2;

/// `_IOC(dir, type, nr, size)`, as the macro computes it: a plain OR of
/// shifted fields, with no masking.
///
/// The lack of masking is deliberate and matters once. A header that
/// passes a number wider than eight bits — MediaTek's `hwtcon` does, for
/// one command — gets a value whose ninth bit lands in the type field,
/// and the kernel compares against that same overflowed value. A helper
/// that masked would compute a number the kernel has never heard of. So
/// `nr` is `u32` here and only the payload size is checked, because
/// fourteen bits of size is a hard limit of the encoding rather than a
/// convention.
pub const fn ioc(dir: u32, ty: u8, nr: u32, size: usize) -> u32 {
    // A payload that does not fit is not an ioctl, and a `const fn` that
    // panics is a build failure, which is the right time to find out.
    assert!(size < (1 << IOC_SIZEBITS));
    (dir << IOC_DIRSHIFT)
        | ((size as u32) << IOC_SIZESHIFT)
        | ((ty as u32) << IOC_TYPESHIFT)
        | (nr << IOC_NRSHIFT)
}

/// `_IO(type, nr)`: an ioctl with no payload.
#[cfg_attr(not(feature = "mxcfb"), allow(dead_code))]
pub const fn io(ty: u8, nr: u8) -> u32 {
    ioc(IOC_NONE, ty, nr as u32, 0)
}

/// `_IOR(type, nr, T)`: the kernel writes a `T` back to user space.
pub const fn ior<T>(ty: u8, nr: u8) -> u32 {
    ioc(IOC_READ, ty, nr as u32, size_of::<T>())
}

/// `_IOW(type, nr, T)`: user space hands the kernel a `T`.
pub const fn iow<T>(ty: u8, nr: u8) -> u32 {
    ioc(IOC_WRITE, ty, nr as u32, size_of::<T>())
}

/// `_IOWR(type, nr, T)`: both directions through one `T`.
pub const fn iowr<T>(ty: u8, nr: u8) -> u32 {
    ioc(IOC_READ | IOC_WRITE, ty, nr as u32, size_of::<T>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoding_reproduces_the_kernels_numbers() {
        // Worked by hand from asm-generic/ioctl.h: direction in the top
        // two bits, size in the next fourteen, 'F' is 0x46.
        assert_eq!(io(b'F', 0x35), 0x0000_4635);
        assert_eq!(iow::<u32>(b'F', 0x2D), 0x4004_462D);
        assert_eq!(ior::<i32>(b'F', 0x31), 0x8004_4631);
        assert_eq!(iowr::<[u8; 8]>(b'F', 0x2F), 0xC008_462F);
    }

    #[test]
    fn an_overflowing_number_lands_where_the_c_macro_puts_it() {
        // nr 0x130 with type 'F' (0x46): the C macro ORs 0x130 into the
        // low bits, so bit 8 sets the low bit of the type field and the
        // kernel sees type 'G', nr 0x30. The number a masking helper
        // would compute, 0x80704630, is one the kernel never matches.
        assert_eq!(ioc(IOC_READ, b'F', 0x130, 112), 0x8070_4730);
    }
}
