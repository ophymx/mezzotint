# Contributing

mezzotint is the layer between "I have rendered RGBA pixels and a damage
rect" and "the glass shows them." On a normal screen that layer is a
present call. On e-ink it is a negotiation, and this crate exists to hold
the part of that negotiation which is not device-specific.

Read [README.md](README.md) for what the crate is and is not. This file is
the part a contributor needs that the code does not explain on its own:
where the facts come from, which licenses may touch them, and what has to
be true before a change lands.

## Verifying

Every change must pass, in this order:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features   # CI sets RUSTFLAGS=-D warnings
cargo test --all-features
cargo deny check licenses                   # policy in deny.toml
```

`cargo test` must stay meaningful with no hardware attached. See *Testing*
below for what that means in practice, and for the one gate that needs a
virtual machine.

CI runs three things beyond that list.

It walks the whole feature ladder — no features, `alloc`, `std`,
everything — rather than only the top rung. The claim that the vocabulary
is core-only and the driver needs nothing but `alloc` is worth exactly as
much as the build that checks it, and it is the single largest lever on
this crate being usable outside the consumer it was written for.

It cross-checks a spread of targets. `armv7` and `aarch64`, because the
backends are Linux-only and the devices are ARM, so a host-only build
proves less than it looks — and because the kernel structs a backend
transcribes embed `unsigned long`, whose width differs between the two.
Windows and macOS, because every other rung is Linux, and that is how the
crate once became uncompilable everywhere else: `fbdev` is on by default,
`libc` is a Linux-only dependency, and nothing gated the module on the
target, so a `cargo add` on either platform failed on the first build with
nothing here able to notice. **A backend gated on a feature must be gated
on its target too** — then the module simply disappears instead of
failing, which is what keeps `fbdev` safe to leave in `default`. And a
bare-metal `thumbv7em`, because a target with no `std` to fall back on is
the only thing that actually proves the `no_std` claim; the feature ladder
above it only proves the features are wired up.

And it builds and tests on the MSRV named by `rust-version` in
`Cargo.toml`, which is a measured number rather than an aspiration: that
entry and the toolchain in `.github/workflows/ci.yml` are two copies of
one fact and move together. The comment beside the manifest entry records
which half of the crate actually needs the version, which is not the half
you would guess.

## Provenance and licensing

This crate is MIT OR Apache-2.0, and keeping it that way is the constraint
that most easily gets broken by accident, because the best documentation
of e-ink refresh behaviour is GPL.

**Sources that may be read for facts but never copied from.** FBInk is
GPLv3, KOReader is AGPL, kobo-rs is GPL-3.0. Which struct version a given
model needs, which waveform constant maps to which mode, which quirk bites
on which panel — those are facts, and facts are not copyrightable. The
expression around them is. Read them to learn what is true, then write the
code from the kernel headers.

**Sources that may be adapted, with attribution.** Two Rust projects cover
adjacent ground under MIT:

- [`libremarkable`](https://github.com/canselcik/libremarkable) — an
  mxcfb layer for the reMarkable, including the ioctl structs, the
  waveform constants, and an `mxcfb_update_data` carrying the fields this
  crate's contract deliberately does not name (`temp`, `dither_mode`,
  `quant_bit`, `update_mode` — see *Decisions that are settled* for why
  each is the backend's to derive).
- [`kobo-core`](https://github.com/Nayeem170/kobo-core) — a Kobo device
  SDK with a table of some twenty models across NXP, sunxi and MTK parts.
  Looked at for the device table and not used: it keys on the `$PRODUCT`
  codename alone, which is ambiguous between hardware revisions that
  differ in ABI, and it carries frontlight, battery and touch details
  rather than anything about the display interface.

MIT permits adaptation but is not free: the copyright notice and
permission text have to travel with anything derived from them. So prefer
transcribing from the kernel UAPI where the facts are identical and the
obligation is zero, and reach for these two where they add something a
header does not have — a device table, a quirk, a model mapping. When you
do adapt, say so at the site and carry the notice.

**The clean base is the kernel UAPI.** `mxcfb.h` and vendor-published
kernels, transcribed with the header and kernel version cited in a comment
next to the struct. A transcription that drifts does not fail loudly; it
reads plausible garbage out of the wrong offsets. Guard every transcribed
struct with `const` assertions on `offset_of!` and `size_of!`, on both
32-bit and 64-bit arms, because every e-ink device is one or the other.

**No NOTICE file yet, deliberately.** The dependency graph is `libc`, which
is MIT OR Apache-2.0, so there is nothing a distributed binary carries that
the crate's own dual license does not already cover. `deny.toml` is what
keeps that true: a dependency arriving under a license not on its allow
list fails the build rather than surfacing at release time. When the graph
grows past the point where one sentence covers it, write the NOTICE.

## Testing

**Everything above the ioctl boundary is tested without hardware, and that
is not a nice-to-have.** Refresh behaviour is both easy to get wrong and
impossible to eyeball: nobody can tell, by looking at an ereader, that a
drag issued one update per pixel of travel. The recording panel, the
simulated panel and the driver tests are the conformance suite, and they
run on a build machine with no panel attached.

**The simulated panel is the one that keeps time, and the fault log is
its point.** `SimulatedPanel` stages on `blit`, reduces on `submit`, and
lands pixels on its glass only when the update completes on a clock the
test advances — a bounded `wait` spends the timeout, an unbounded one
spends whatever is left. It records a `Fault` for every write under a
region the driver asked it to refresh, which is the rule the driver
exists to hold and which no earlier double could check from the panel's
side. A shell author drives their own code against it and asserts that
`faults` is empty. When an EPDC backend lands, the same tests run against
it with a real clock; the simulation is what the device is diffed
against, not a substitute for it. It models nothing that is only
knowable on glass: no ghosting appearance, no temperature, no waveform
artifacts.

`viewer/` is that panel in a window, with wall time as its clock and a
stand-in shell driving it. It is a separate crate rather than an example
because a windowing stack is a few hundred crates across every platform
and `deny.toml` scans every platform, so it must not join the library's
graph. Its shell shows the one thing a real one has to get right on a UI
thread: it never blocks on the panel. Damage over a live region waits for
the next frame, and a zero-deadline `flush_within` each frame is the poll
that retires what finished. CI builds and lints it so it cannot rot.

**The ioctl boundary itself gets a thin, obviously-transcribed layer plus
on-device smoke tools** — `examples/` binaries a person runs while holding
the device. Code that runs is not the same as code that is right, and for
a waveform the difference is only visible on glass.

**A framebuffer's pixel format comes from hardware, so a unit test can only
assert constants somebody typed.** QEMU supplies real ones: a host kernel,
an initramfs, and `vga=` to pick a mode. `scripts/fbdev-vm.sh [vga-mode]`
is that harness. That is how the packing and the palette rejection get
checked against a kernel instead of against an assumption.

A harness like that is only a gate if it can fail, and the shape it wants
to have is the shape that cannot. A script that ends by `exec`-ing qemu
reports qemu's exit status, and a self-test result printed to the serial
console is something a human reads and CI does not — so wiring one up
unchanged produces a step that passes unconditionally. The script
therefore captures the console, greps it for the exit lines, fails when
they are missing as well as when they are non-zero, and puts a timeout
around the boot so a hung guest fails rather than hangs. Keep all four of
those if you touch it.

**And a pass has to be a pass for the right reason.** The mode that must
be *refused* — an 8bpp palette visual, whose pixels are indices rather
than colours — was the second place this went wrong, and more quietly than
the first. Any non-zero exit from `probe` was being read as "the backend
declined it", which is true on a kernel that gives you such a framebuffer
and false on one that does not: a kernel binding `simpledrm` rather than
`vesafb` will not drive a palette mode at all, so there is no `/dev/fb0`,
`probe` fails because the device is missing, and the rejection under test
never runs. It passed in CI that way.

Two things fix it and both are worth keeping. The guest reports whether
`/dev/fb0` existed, and an absent device is a failure rather than a skip —
a mode this kernel cannot produce tests nothing, and saying so out loud is
the whole difference between a gate and a green tick. And `probe` exits
**3** specifically for "this backend declines these pixels", so the script
asserts on that code rather than on "non-zero", which cannot be confused
with any other way of failing.

CI therefore runs RGB565 and 24bpp, which its kernel does produce, and
*not* the palette mode, which it cannot — run that one locally, on a host
with a `CONFIG_FB_VESA` kernel. What stands in for it there is a negative
control that needs no palette device: `MUST_REFUSE=yes` on a mode that
must be accepted, which the harness has to fail. If the refusal branch
ever stops being able to fail, that is what says so.

## Backends

One feature per SoC driver family, not per vendor. `mxcfb-kobo` reads
naturally right up until `mxcfb-kindle` arrives sharing ninety percent of
the code and a third device has to pick a side. The ioctl world is the
feature; the vendor is a row in a device table, which is runtime data.

**An EPDC backend is hardware-gated, and that is a rule rather than a
preference.** `mxcfb` is not a generic e-ink API — it is NXP's driver for
one SoC family, its struct layouts and waveform constants vary per vendor
*and* per model, and every quirk worth having was found by somebody
holding the device. Code written without one runs; running is not the same
as being right, and for a waveform the difference is only visible on glass.

When one is written, keep three things apart, because that is where the
per-model variation concentrates: the ioctl surface, the waveform table,
and the device-identification table. The first exists — `src/mxcfb/`
transcribes NXP's UAPI header with every offset asserted, and its module
docs record the struct shapes other vendors ship, including the one that
shares a size and an ioctl number with NXP's and differs in layout. The
third exists too — `src/devices.rs` maps Kobo's own device ID, codename
and mainline device-tree string to the SoC and the display interface, as
data, so adding a model is adding a row. Its IDs and codenames are Kobo's
(Nickel's version tag and `kobo_config.sh`), consolidated from FBInk and
KOReader as facts with nothing taken from either. The waveform table is
what remains, and it is the hardware-gated part.

After i.MX, the same trait covers Allwinner-Kobo (a sunxi shim, a
different ioctl world), the Kindle mxcfb variants, and MTK. The reMarkable
2 has no framebuffer at all and is out of scope until somebody owns one.

## Constraints that outlive any one file

These are the conclusions that cost the most to reach, kept here because
they are true of the crate rather than of a particular module.

**Error diffusion is not local, but it is not global either.** A
Floyd–Steinberg pass scoped to an arbitrary damage rect starts from zero
error at its edges and leaves a visible seam against the surrounding
field. The naive conclusion — diffuse only over the whole surface — is
too strong. Diffusion is safe within a *content-bounded* region, because
an image's boundary is already a hard content edge and hides the
discontinuity that the same seam would advertise mid-paragraph. The
useful shape is therefore not one dither flag per surface but a set of
regions that may be diffused within; one flag for a whole page is wrong,
and at two levels it is decisively wrong, since diffusing through body
text stipples the antialiased edge of every glyph while not diffusing
through a photograph turns it into a silhouette.

**Which regions those are is knowledge this crate cannot have.** mezzotint
takes buffers and rects and never a document, so it cannot tell a
photograph from a paragraph. Whatever the encode path ends up looking
like, it must let the consumer say where diffusion belongs, because the
consumer is the only party that knows.

**Depth belongs to the update, not to the session.** A panel whose depth
varies per waveform — every EPDC — can only reduce once the update class
is known. Reducing above the panel is wrong in both directions: flatten to
sixteen levels and the fast update re-flattens it anyway, flatten to two
and the next page turn is ruined.

**Rects round outward, and a refresh may cover more than it was asked to,
never less.** A region trimmed by half a pixel leaves a stale sliver, and
on e-ink a stale sliver stays there until something else disturbs it.
Widening only costs time.

**More than one update can be in flight.** A driver that tracks a single
outstanding update silently forgets the earlier one as soon as a
non-overlapping update is submitted, and the next write over the forgotten
region proceeds while the controller is still reading it. That is exactly
the tearing the rule against writing under a live update exists to
prevent. Track the set, retire entries on wait.

**Mono polarity comes from the device, never from a guess.** Getting it
backwards produces a perfectly formed negative image and no error at all.
The kernel's `visual` says which bit means white; bit order within the
byte is MSB-first, which nothing reports and every mono fbdev driver
assumes.

**The asynchronous half of the contract has never run on hardware.** The
only device backend that exists is a framebuffer, whose `submit` is a
no-op and whose `wait` returns immediately. `SimulatedPanel` is the first
backend that genuinely keeps an update in flight, and `PanelDriver`'s
collision handling is now proven against it — the panel reports, from its
own side, that nothing wrote under a live update — as well as against
`RecordingPanel` and the doubles beside it. That is still a model of a
controller and not a controller. Treat it as tested rather than as
field-proven, and expect the first EPDC backend to find something.

That is not hypothetical: the single-slot version of that same code was
green for its whole life and was still wrong in two ways, both of which
only bite once a submit actually queues. `a_third_update_does_not_write_
under_the_first` and `flush_waits_for_every_live_update` are named for
what they protect; do not let either become an assertion about something
else.

## Decisions that are settled

These cost more to change than to make, because each one is a breaking
change to something every backend or consumer touches. They are recorded
here so they are not relitigated from scratch, and so the *reason* is
available to whoever eventually has cause to revisit one.

**`blit` takes a positioned view, not a whole-panel buffer.** `Source`
carries its own origin, extent and stride. A page-oriented consumer keeps
one buffer and hands over `Source::panel`, exactly as before; an embedded
consumer driving a 400x300 panel updates a status line without a 480 KiB
whole-panel allocation it may not have. Widening this after publication
would have been a breaking change to the one method every backend
implements.

That has one consequence worth stating, because it is not obvious and the
driver hides it: a *scheduled* flash repaints the whole panel, and a
consumer holding only the strip it repainted does not have the rest of it.
`PanelDriver` defers the escalation there rather than failing — the debt
stays owed and the next update with a whole panel behind it pays it —
because an error arriving on a ghosting cadence the consumer never asked
about is a poor reward for having taken the cheaper convention. A flash
the consumer asked for itself is *not* deferred; that one is a caller bug
and gets an error naming the region it was short of.

**Lending the staging buffer is deferred, not precluded.** Whether a
backend can hand *out* its mapped memory so a consumer renders straight
into it wants a measurement on a real framebuffer rather than a guess.
Nothing forecloses it: it arrives later as a provided method on `Panel`
returning `Option<&mut [u8]>`, and adding a provided method breaks nobody.

**`submit` takes an `Update`, not a bare class.** `Update`'s fields are
private and grow behind builder methods, so a hint that turns out to be
needed arrives without breaking the one trait method every backend
implements, and a backend that only reads the class keeps compiling. That
is the mechanism. What is allowed through it is the next decision.

**A hint enters `Update` only when the consumer knows something the
backend cannot derive.** The test: does a shell above the seam
legitimately know this, per update, in terms that mean the same thing on
every controller? The class, the rect and the diffusion regions pass it.
The four fields of `mxcfb_update_data` (NXP `include/uapi/linux/mxcfb.h`)
that looked like gaps were each held to it, and none pass — so none is
coming, and a backend author should not wait for them:

- `update_mode` (partial or full) *is* the class. `Flash` is a full-mode
  update of the whole panel; everything else is partial. That the
  constant sits beside the waveform rather than inside it is an mxcfb
  spelling, not a second axis a consumer could set independently.
- `temp` is neither per update nor the shell's. The controller's own
  sensor (`TEMP_USE_AMBIENT`) is the right answer on a working device,
  and an explicit temperature is a quirk for one whose sensor is not —
  a row in the device table, or at most a constructor option on the
  backend. Nothing above the seam can know a better number.
- `quant_bit` is the depth of the waveform the class mapped to, which
  the backend's own waveform table already holds. Asking the consumer
  would be asking for the answer to a question it cannot see.
- `dither_mode` follows from the diffusion regions, and this derivation
  is the one that needs care. Hardware dithering runs over the whole
  region the controller refreshes and cannot be scoped inside it. So it
  is usable exactly when the regions the consumer named cover the rect
  the backend actually submits — which, after alignment widening, may be
  more than the rect it was asked for. An empty set is quantize-only.
  Anything in between is software diffusion inside the regions, in
  `encode`, with the hardware told to leave the result alone. Older
  kernels spell the same choice as `EPDC_FLAG_USE_DITHERING_Y1`/`Y4`
  flags, and the derivation is the same. Which kernel the hardware runs
  when it does run — Floyd–Steinberg, Atkinson, ordered — is a backend
  choice the consumer never asked to make; if one ever needs to, that is
  a builder method, not a redesign.

The remaining `flags` fall the same way: `FORCE_MONOCHROME` is
`Monochrome`, `USE_REGAL` is `Quality` on a model that has it, and
collision testing and update grouping are the backend's own bookkeeping.
`ENABLE_INVERSION` is the only one a shell could plausibly want, and it
is a property of the pixels, which the consumer already owns.

**`wait` takes a deadline.** `Option<Duration>`, returning `Wait` rather
than treating expiry as an error — a caller polling with a zero timeout is
not in an exceptional state. mxcfb's `WAIT_FOR_UPDATE_COMPLETE` blocks
indefinitely on a wedged controller, and a trait with no way out leaves a
shell nothing to do but hang.

**`UpdateClass` has no `None`.** "Nothing to show" is the absence of an
update, not a kind of one. Keeping it would make every backend write a
match arm for a case the driver filters out before `submit` is ever
called. Where a consumer's own arithmetic produces "no change", that is an
`Option<UpdateClass>` on the consumer's side.

**`UpdateToken` wraps a private field.** It is documented as opaque, so it
had better be; `new` and `raw` are what a backend needs, and the type is
now free to stop being a `u32`.

**`PanelInfo` and `PanelRect` keep public fields and are not
`#[non_exhaustive]`.** Backends live outside this crate and *construct*
both, and sealing construction would make a third-party backend impossible
to write. Adding a field is a breaking change; that is the trade, taken
deliberately, because the alternative costs something every day rather
than once.

**The error type is a small `#[non_exhaustive]` enum, structured rather
than stringly typed.** A backend runs on a device with no terminal
attached, so the numbers a diagnosis needs — the depth the kernel
reported, the visual it claimed — have to survive formatting. A `String`
message would also put the whole vocabulary behind `alloc` for the sake of
error text.

**Both channels for stating where diffusion belongs stay, for now.**
`PanelInfo::format` asks the consumer to pre-reduce; `Update::dithering_
within` lets a panel that stages do the reduction itself at submit, which
keeps per-update depth *and* region-aware diffusion at the same time —
something the format request cannot do. The second is the better shape and
the one to build on. It cannot retire the first until a backend that
stages exists, because a framebuffer puts its pixels on screen during
`blit`, one call before the diffusion regions arrive. That is a decision
for the first EPDC backend to make, with hardware in front of it.
