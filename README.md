# mezzotint

Getting rendered pixels onto e-ink panels and Linux framebuffers,
correctly. An electrophoretic display is not a swapchain: it is *asked* to
change, and how it is asked — which waveform, which region, whether to
flash — decides how the change looks and feels. mezzotint owns that
conversation, from a vendor-neutral update vocabulary down to the ioctls,
so the thing producing the pixels never has to know what an EPDC is.

Named for the printmaking technique invented to reproduce continuous
greyscale tone — which is this crate's whole job.

## What it is

- A **panel contract**: submit a damage rect with an *update class*
  (monochrome-fast → full-flash), get a token, wait on it when you must.
  Classes name the kind of change; each backend maps them to its own
  waveforms.
- A **driver** that enforces the rules that work on a desk and fail on a
  device: never write under an in-flight update, repaint what a fast
  update degraded, ration the ghost-clearing flash.
- **Backends**: an in-memory recording panel and a simulated EPDC that
  keeps updates in flight on a clock the test owns (both for CI), the
  generic Linux framebuffer (fbdev), and — the reason the crate exists —
  EPDC devices, starting with i.MX/mxcfb e-readers.
- The **encode path**: RGBA in; luminance, quantization and error
  diffusion in `mezzotint::encode`, packing inside each backend, per
  update where the hardware allows it.

## What it is not

- Not a renderer. Bring your own pixels; mezzotint has no fonts, no text,
  no drawing.
- Not a device SDK. Input, frontlight, and power management are a
  different concern for a different crate.
- Not a UI toolkit.

## The rules it holds

These are the things an application otherwise reimplements from folklore,
and each is easier to get wrong than to notice:

- **Update classes, not waveforms.** You say what *kind* of change this is
  — a selection tracking under a finger, a page turn, a full reflow. The
  backend says what its controller calls that. Escalating is always safe.
- **blit, submit and wait are three operations.** A refresh takes 100ms to
  a second. Folding the wait into the submit is what makes a page turn
  feel broken.
- **Depth belongs to the update, not the session.** Two levels for a fast
  waveform, sixteen for a full one — so reduction happens at submit, the
  first moment the class is known, and not above the panel where it could
  only be right for one of them.
- **Damage rects round outward and may be widened, never narrowed.** A
  region trimmed by half a pixel leaves a stale sliver, and on e-ink a
  stale sliver stays there until something else disturbs it.
- **Nothing writes under a live update.** The controller is reading that
  memory. The driver waits on genuine overlap and only on genuine overlap,
  so an unrelated corner of the screen never pays for a slow refresh
  elsewhere.
- **Fast updates leave debt.** A monochrome update marks its region
  provisional and the driver repaints it when you say the gesture ended;
  ghosting accrues and the full flash is rationed rather than avoided.
- **Where error diffusion belongs is yours to say.** Diffusion is not
  local, and it is not global either: scoped to an arbitrary damage rect
  it seams at the edges, but inside a photograph — whose boundary is
  already a hard content edge — it is exactly right. Only you know which
  regions those are, so you name them, per update.

Everything above the ioctls is testable with no hardware attached: the
recording panel, the simulated panel and the driver tests are the
conformance suite, and they run on a build machine with no panel. The
simulated panel is the one that keeps time — pixels reach its glass when
an update completes, not when it is submitted — so it can say from its
own side whether anything wrote under a live update, which is the
assertion the driver exists to satisfy. `viewer/` puts that panel in a
desktop window with wall time as its clock, so a shell can be watched as
well as asserted about: page turns land after their latency, a drag's
monochrome strips and the repaint on release are visible, the regions
the controller is busy with are outlined, and the fault count sits in
the title bar. The one thing a unit test cannot
reach — what pixel format a real kernel hands you — is checked by booting
one under QEMU, at RGB565, at 24bpp, and at the palette visual the fbdev
backend must refuse.

## Using it

```rust
use mezzotint::{PanelDriver, PanelRect, Source, UpdateClass};

let mut driver = PanelDriver::new(panel);
let info = driver.info();

// A page turn: full fidelity, no flash unless the ghost debt is due.
let src = Source::panel(&page, info.width, info.height)?;
driver.present(src, None, UpdateClass::Quality)?;

// A selection following a finger: fast and ugly, and the driver
// remembers it owes the region a clean pass.
let line = PanelRect::new(0, 40, info.width, 20);
driver.present(src, Some(line), UpdateClass::Monochrome)?;

// The finger lifted. Only your shell knows that.
driver.settle(src, &[])?;
driver.flush()?;
```

`Source` positions its pixels on the panel itself, so a consumer that
keeps one page buffer passes `Source::panel` and a consumer that rendered
only the strip that changed passes `Source::at` — the backend's loop is
the same either way, and neither has to allocate for the other's habits.

## Features

| feature | what it adds |
|---|---|
| `alloc` | `PanelDriver`, `RecordingPanel` and `SimulatedPanel` |
| `std` (default) | `std::io::Error` inside the error type, and backends |
| `fbdev` (default) | the Linux framebuffer backend |
| `mxcfb` | the i.MX EPDC ioctl vocabulary, transcribed and layout-checked; not yet a backend |

The vocabulary — update classes, rects, the `Panel` trait, the refresh
policy, the encoder, the device table — is `core`-only and allocates
nothing. Turn
everything off and it builds for a microcontroller, which is the point:
the audience the `embedded` keyword advertises to cannot use a `std`-only
crate at all.

## Where it sits

The Rust e-ink neighbourhood, so you can tell quickly whether this is the
crate you want:

- [`epd-waveshare`](https://github.com/rust-embedded-community/epd-waveshare)
  drives SPI panels through embedded-graphics. If your panel hangs off a
  microcontroller's SPI bus, start there.
- [`libremarkable`](https://github.com/canselcik/libremarkable) does the
  Linux EPDC seam well, for one device, bundled with input, power and a UI
  framework.
- [`kobo-core`](https://github.com/Nayeem170/kobo-core) is a full Kobo
  device SDK — framebuffer, touch, power, audio, formats — with a device
  database covering twenty models.

mezzotint is the device-agnostic Linux EPDC layer and *only* that layer:
no device ownership, no input, no rendering, one trait a backend
implements and a driver that knows the rules.

Status: 0.1. The contract, the driver, the recording panel and the fbdev
backend are here and tested; the decisions that would be expensive to
change after publication — the `no_std` split, the blit buffer
convention, the shape of `submit`, the deadline on `wait` — are made and
written down in [CONTRIBUTING.md](CONTRIBUTING.md).

What is *not* here is the reason the crate exists: an EPDC backend.
`mxcfb` lands when there is hardware in hand to write it against, as
0.2's headline. Waveform behaviour is only visible on glass, and a
version of it written from headers alone would run without being right.
What headers *can* give — the i.MX ioctl structs and numbers, with every
offset asserted — is already in, behind the `mxcfb` feature, so the
backend starts from a checked vocabulary rather than from a header.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the provenance and licensing
discipline, which is stricter than it looks — most of the good
documentation of e-ink refresh behaviour is GPL, and this crate is not.

## License

MIT OR Apache-2.0.
