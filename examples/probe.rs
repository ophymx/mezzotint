//! Report what a framebuffer says about itself, without drawing on it.
//!
//!     cargo run --example probe -- [/dev/fb0]
//!
//! The first thing to run when bringing up a device, and the only part of
//! this backend that can be checked without taking over the screen: if the
//! geometry here is wrong, the `#[repr(C)]` structs do not match the
//! kernel's and nothing below them can be trusted.

use mezzotint::fbdev::FbdevPanel;
use mezzotint::{Error, Panel};

/// Exit codes, because "it failed" is not a result a harness can check.
///
/// A device this backend deliberately declines and a device that is not
/// there both leave a non-zero status behind, and they mean opposite
/// things: the first is the rejection working, the second is the
/// rejection never having run. `scripts/fbdev-vm.sh` asserts the
/// difference, so it has to be visible from outside the process.
const REFUSED_PIXELS: i32 = 3;
const REFUSED_GEOMETRY: i32 = 4;

fn main() {
    let device = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/fb0".to_string());
    let panel = match FbdevPanel::open_path(&device) {
        Ok(panel) => panel,
        Err(err) => {
            println!("{device}: {err}");
            std::process::exit(match err {
                Error::UnsupportedPixels { .. } => REFUSED_PIXELS,
                Error::UnsupportedGeometry { .. } => REFUSED_GEOMETRY,
                _ => 1,
            });
        }
    };
    let info = panel.info();
    println!("{device}: {}", panel.describe());
    println!(
        "  asks for {}x{} {:?}",
        info.width, info.height, info.format
    );
}
