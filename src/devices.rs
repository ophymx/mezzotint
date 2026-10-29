//! Which e-reader this is, and therefore which display interface it
//! speaks.
//!
//! An EPDC backend cannot pick its ioctl surface from the ioctl surface:
//! the same `MXCFB_SEND_UPDATE` number is answered by kernels expecting
//! different struct layouts (see [`mxcfb`](crate::mxcfb)), and a wrong
//! layout is accepted silently. The only thing that can choose is knowing
//! the device. That knowledge is *data*, and this is where it lives — a
//! table a backend consults, not code a backend contains — so that adding
//! a model is adding a row.
//!
//! # Keys
//!
//! Kobo identifies its own hardware three ways, and a bring-up path tries
//! them in this order:
//!
//! 1. **The device ID**: the last three digits of Nickel's version tag,
//!    `/mnt/onboard/.kobo/version`. [`parse_version_tag`] extracts it and
//!    [`by_id`] looks it up. This is the key that disambiguates: two
//!    revisions of the Aura H2O Edition 2 share a codename and differ in
//!    kernel and therefore in ABI.
//! 2. **The codename**: `$PRODUCT` in the environment, or the output of
//!    `/bin/kobo_config.sh`. [`by_codename`] returns every row that
//!    carries it, because several codenames are ambiguous.
//! 3. **The device-tree compatible string**, on a mainline kernel:
//!    `/sys/firmware/devicetree/base/compatible`, NUL-separated.
//!    [`by_compatible`] handles the `kobo,*` entries; a bare SoC string
//!    such as `fsl,imx6sll` identifies a family and not a device, and a
//!    backend decides for itself what to assume.
//!
//! Reading any of those needs `std` and belongs to the backend. Nothing
//! here does I/O.
//!
//! # Provenance
//!
//! The IDs and codenames are Kobo's own, as Nickel and `kobo_config.sh`
//! emit them. They and the generation each model belongs to were
//! consolidated from the two projects that are the working references for
//! Kobo hardware, FBInk and KOReader, both read for facts only — which
//! model has which ID, which kernel it runs, which ioctl world that puts
//! it in — with nothing taken from either. The `kobo-core` table that
//! `CONTRIBUTING.md` once pointed at was looked at and not used: it keys
//! on codename alone and carries no display-interface column, which is
//! the one column this table exists for.

/// The system-on-chip family, which is what decides the kernel and so the
/// ioctl world.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Soc {
    /// Freescale i.MX507. The 2.6.35 kernel.
    Imx50,
    /// NXP i.MX6 SoloLite, SoloLiteLite and ULL. Two kernel generations
    /// ran on these, which is why the SoC alone does not name the ABI.
    Imx6,
    /// Allwinner B300 (sun8iw15p1).
    SunxiB300,
    /// MediaTek MT8113.
    Mt8113,
}

/// The display interface a device exposes, which is what a backend has to
/// match exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Display {
    /// NXP's EPDC driver through `mxcfb`, in the layout NTX's kernels
    /// carried before Mark 7: `mxcfb_update_data` **without**
    /// `dither_mode` and `quant_bit`, and with a kernel pointer as the
    /// first field of its alt-buffer data — 68 bytes on the 32-bit
    /// devices that have it, so `MXCFB_SEND_UPDATE` is `0x4044462E`.
    /// The wait takes a bare `__u32` marker: `_IOW('F', 0x2F, __u32)`,
    /// `0x4004462F`. Not transcribed in [`mxcfb`](crate::mxcfb) because no
    /// citable header for it is published loose; it is described there.
    MxcfbNtx,
    /// NXP's EPDC driver through `mxcfb` as
    /// [`mxcfb::uapi`](crate::mxcfb::uapi) transcribes it: the 72-byte
    /// update with dither fields and the marker-data wait. Kobo Mark 7
    /// and later i.MX6 devices, running the 4.1 kernel.
    Mxcfb,
    /// Allwinner's `disp` driver. Not mxcfb at all; a different ioctl
    /// world with its own update and wait.
    SunxiDisp,
    /// MediaTek's `hwtcon` driver: `mxcfb`'s letter and most of its
    /// numbers around a smaller update struct. The `hwtcon` feature's
    /// backend speaks it.
    MtkHwtcon,
}

/// One e-reader model, as Kobo's own identification names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Device {
    /// Kobo's device ID, the last three digits of the version tag.
    /// Tolino-branded twins are the Kobo ID plus 300.
    pub id: u16,
    /// The codename `$PRODUCT` carries. Shared between hardware revisions
    /// and between Kobo and Tolino twins, so not unique.
    pub codename: &'static str,
    /// The marketing name.
    pub model: &'static str,
    /// The SoC family.
    pub soc: Soc,
    /// The display interface a backend must speak to it.
    pub display: Display,
}

const fn kobo(
    id: u16,
    codename: &'static str,
    model: &'static str,
    soc: Soc,
    display: Display,
) -> Device {
    Device {
        id,
        codename,
        model,
        soc,
        display,
    }
}

use Display::{MtkHwtcon, Mxcfb, MxcfbNtx, SunxiDisp};
use Soc::{Imx50, Imx6, Mt8113, SunxiB300};

/// Every Kobo and Tolino-twin model known, in ID order within family.
///
/// Ordered by hardware generation rather than by ID so that the table
/// reads as the history it is; [`by_id`] does not rely on the order.
pub const KOBO: &[Device] = &[
    // Mark 3 and 4: i.MX507, the 2.6.35 kernel, the NTX mxcfb layout.
    kobo(310, "trilogy", "Kobo Touch", Imx50, MxcfbNtx),
    kobo(320, "trilogy", "Kobo Touch C", Imx50, MxcfbNtx),
    kobo(340, "pixie", "Kobo Mini", Imx50, MxcfbNtx),
    kobo(330, "kraken", "Kobo Glo", Imx50, MxcfbNtx),
    kobo(350, "dragon", "Kobo Aura HD", Imx50, MxcfbNtx),
    // Mark 5 and 6: i.MX6SL, the 3.0.35 kernel, still the NTX layout.
    kobo(360, "phoenix", "Kobo Aura", Imx6, MxcfbNtx),
    kobo(370, "dahlia", "Kobo Aura H2O", Imx6, MxcfbNtx),
    kobo(371, "alyssum", "Kobo Glo HD", Imx6, MxcfbNtx),
    kobo(671, "alyssum", "Tolino Shine 2HD", Imx6, MxcfbNtx),
    kobo(372, "pika", "Kobo Touch 2.0", Imx6, MxcfbNtx),
    kobo(373, "daylight", "Kobo Aura ONE", Imx6, MxcfbNtx),
    kobo(
        381,
        "daylight",
        "Kobo Aura ONE Limited Edition",
        Imx6,
        MxcfbNtx,
    ),
    kobo(374, "snow", "Kobo Aura H2O Edition 2", Imx6, MxcfbNtx),
    kobo(375, "star", "Kobo Aura Edition 2", Imx6, MxcfbNtx),
    kobo(675, "star", "Tolino Vision 4", Imx6, MxcfbNtx),
    // Mark 7: i.MX6SLL and ULL, the 4.1.15 kernel, NXP's own layout. The
    // two "r2" rows are the same product names as the two above them on
    // a new board, which is why the ID and not the codename is the key.
    kobo(378, "snow", "Kobo Aura H2O Edition 2 r2", Imx6, Mxcfb),
    kobo(379, "star", "Kobo Aura Edition 2 r2", Imx6, Mxcfb),
    kobo(376, "nova", "Kobo Clara HD", Imx6, Mxcfb),
    kobo(676, "nova", "Tolino Shine 3", Imx6, Mxcfb),
    kobo(377, "frost", "Kobo Forma", Imx6, Mxcfb),
    kobo(380, "frost", "Kobo Forma 32GB", Imx6, Mxcfb),
    kobo(677, "frost", "Tolino Epos 2", Imx6, Mxcfb),
    kobo(384, "storm", "Kobo Libra H2O", Imx6, Mxcfb),
    kobo(684, "storm", "Tolino Vision 5", Imx6, Mxcfb),
    kobo(382, "luna", "Kobo Nia", Imx6, Mxcfb),
    kobo(388, "io", "Kobo Libra 2", Imx6, Mxcfb),
    kobo(386, "goldfinch", "Kobo Clara 2E", Imx6, Mxcfb),
    // Mark 8: Allwinner B300. Not mxcfb.
    kobo(387, "europa", "Kobo Elipsa", SunxiB300, SunxiDisp),
    kobo(383, "cadmus", "Kobo Sage", SunxiB300, SunxiDisp),
    kobo(683, "cadmus", "Tolino Epos 3", SunxiB300, SunxiDisp),
    // MediaTek. Not mxcfb either.
    kobo(389, "condor", "Kobo Elipsa 2E", Mt8113, MtkHwtcon),
    kobo(390, "monza", "Kobo Libra Colour", Mt8113, MtkHwtcon),
    kobo(690, "monza", "Tolino Vision Color", Mt8113, MtkHwtcon),
    kobo(391, "spaBW", "Kobo Clara BW", Mt8113, MtkHwtcon),
    kobo(691, "spaBW", "Tolino Shine BW", Mt8113, MtkHwtcon),
    kobo(393, "spaColour", "Kobo Clara Colour", Mt8113, MtkHwtcon),
    kobo(693, "spaColour", "Tolino Shine Color", Mt8113, MtkHwtcon),
    kobo(395, "spaBW", "Kobo Clara BW (TPV panel)", Mt8113, MtkHwtcon),
];

/// The device with this ID, if it is known.
pub fn by_id(id: u16) -> Option<&'static Device> {
    KOBO.iter().find(|d| d.id == id)
}

/// Every device that carries this codename. Often one; for `trilogy`,
/// `snow` and `star` more than one, and for the last two the candidates
/// differ in [`Display`], so a caller with only a codename has to go and
/// find the ID.
pub fn by_codename<'a>(codename: &'a str) -> impl Iterator<Item = &'static Device> + 'a {
    KOBO.iter().filter(move |d| d.codename == codename)
}

/// The device a mainline kernel's device-tree names, for the `kobo,*`
/// compatible strings. A SoC-only string is not a device and gives
/// `None`.
pub fn by_compatible(compatible: &str) -> Option<&'static Device> {
    let id = match compatible {
        "kobo,aura" => 360,
        "kobo,aura2" => 375,
        "kobo,clarahd" => 376,
        "kobo,librah2o" => 384,
        "kobo,nia" => 382,
        "kobo,tolino-shine2hd" => 671,
        "kobo,tolino-shine3" => 676,
        "kobo,tolino-vision" => 675,
        "kobo,tolino-vision5" => 684,
        _ => return None,
    };
    by_id(id)
}

/// The device ID from the contents of `/mnt/onboard/.kobo/version`.
///
/// The tag is one comma-separated line whose last field ends in the
/// three-digit ID; the rest of it is serial and firmware versions that
/// change per unit and per update. Trailing whitespace is tolerated. An
/// empty or truncated file — which a crash can leave behind — gives
/// `None` rather than a wrong device.
pub fn parse_version_tag(tag: &str) -> Option<u16> {
    let tag = tag.trim_end();
    if tag.len() < 3 {
        return None;
    }
    let (_, last3) = tag.split_at(tag.len() - 3);
    if !last3.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    last3.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        for (i, a) in KOBO.iter().enumerate() {
            for b in &KOBO[i + 1..] {
                assert_ne!(a.id, b.id, "{} and {}", a.model, b.model);
            }
        }
    }

    #[test]
    fn a_tolino_twin_is_its_kobo_plus_three_hundred_and_speaks_the_same_interface() {
        for t in KOBO.iter().filter(|d| d.model.starts_with("Tolino")) {
            let k = by_id(t.id - 300).unwrap_or_else(|| panic!("{} has no Kobo twin", t.model));
            assert_eq!(k.codename, t.codename, "{}", t.model);
            assert_eq!(k.display, t.display, "{}", t.model);
            assert_eq!(k.soc, t.soc, "{}", t.model);
        }
    }

    #[test]
    fn the_interface_follows_the_generation() {
        for d in KOBO {
            match d.soc {
                Soc::Imx50 => assert_eq!(d.display, Display::MxcfbNtx, "{}", d.model),
                Soc::Imx6 => assert!(
                    matches!(d.display, Display::MxcfbNtx | Display::Mxcfb),
                    "{}",
                    d.model
                ),
                Soc::SunxiB300 => assert_eq!(d.display, Display::SunxiDisp, "{}", d.model),
                Soc::Mt8113 => assert_eq!(d.display, Display::MtkHwtcon, "{}", d.model),
            }
        }
    }

    #[test]
    fn a_shared_codename_is_why_the_id_is_the_key() {
        let snow: alloc_free::Vec<_> = by_codename("snow").map(|d| d.display).collect();
        assert_eq!(snow, [Display::MxcfbNtx, Display::Mxcfb]);
        let star: alloc_free::Vec<_> = by_codename("star").map(|d| d.display).collect();
        assert_eq!(star, [Display::MxcfbNtx, Display::MxcfbNtx, Display::Mxcfb]);
        assert_eq!(by_codename("nova").count(), 2);
        assert_eq!(by_codename("nothing").count(), 0);
    }

    #[test]
    fn the_version_tag_ends_in_the_id() {
        assert_eq!(
            parse_version_tag("N4110000000000,4.2.0,4.38.23012,4.2.0,4.2.0,00000000-0000-0000-0000-000000000376\n"),
            Some(376)
        );
        assert_eq!(by_id(376).unwrap().model, "Kobo Clara HD");
        assert_eq!(parse_version_tag(""), None);
        assert_eq!(parse_version_tag("37"), None);
        assert_eq!(parse_version_tag("...-0000000003x6"), None);
    }

    #[test]
    fn a_mainline_device_tree_names_the_device_or_nothing() {
        assert_eq!(by_compatible("kobo,clarahd").unwrap().id, 376);
        assert_eq!(
            by_compatible("kobo,aura2").unwrap().model,
            "Kobo Aura Edition 2"
        );
        assert_eq!(by_compatible("fsl,imx6sll"), None);
    }

    // The tests above collect into a Vec, which `core` does not have; the
    // crate's own `alloc` gate is not on for a `--no-default-features`
    // test run, so give the tests a Vec the cheap way.
    mod alloc_free {
        extern crate alloc;
        pub use alloc::vec::Vec;
    }
}
