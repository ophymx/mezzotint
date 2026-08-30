//! Report what a framebuffer says about itself, without drawing on it.
//!
//!     cargo run --example probe -- [/dev/fb0]
//!
//! The first thing to run when bringing up a device, and the only part of
//! this backend that can be checked without taking over the screen: if the
//! geometry here is wrong, the `#[repr(C)]` structs do not match the
//! kernel's and nothing below them can be trusted.

use mezzotint::fbdev::FbdevPanel;
use mezzotint::Panel;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let device = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/fb0".to_string());
    let panel = FbdevPanel::open_path(&device)?;
    let info = panel.info();
    println!("{device}: {}", panel.describe());
    println!(
        "  asks for {}x{} {:?}",
        info.width, info.height, info.format
    );
    Ok(())
}
