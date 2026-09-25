//! A [`Panel`] that behaves like an electrophoretic controller *in time*.
//!
//! The recording panel says what a shell asked for. This one says what
//! the glass would show, and when — which is the half of the contract no
//! other backend has ever exercised: every double so far completes at
//! submit, so the driver's rule against writing under a live update has
//! only been checked by looking at the order of `wait` calls. Here the
//! panel knows which regions it is still reading, and a write under one
//! is recorded as a [`Fault`]. That is the assertion the driver exists
//! to satisfy, checkable on a build machine.
//!
//! # The model
//!
//! Only what an EPDC does that a framebuffer does not, and nothing that
//! is only knowable on glass:
//!
//! - **Staging and glass are different buffers.** [`blit`](Panel::blit)
//!   writes staging. [`submit`](Panel::submit) snapshots the region and
//!   reduces it for the class. The pixels reach the glass when the update
//!   *completes*, so a test can ask what is visible at a given moment.
//! - **The clock belongs to the test.** No real time: this is `core`
//!   plus `alloc`, and a test that sleeps is a test that flakes.
//!   [`advance`](SimulatedPanel::advance) moves time; a bounded
//!   [`wait`](Panel::wait) moves it by at most the timeout, which is what
//!   blocking is; an unbounded one moves it to completion.
//! - **Each class takes a different time**, from [`SimConfig`]. Defaults
//!   are plausible i.MX figures, and only their ordering matters.
//! - **The region is widened to an alignment**, never narrowed, as
//!   controllers do. What was asked for and what was refreshed are both
//!   recorded.
//! - **An update queues behind whatever it overlaps.** The controller
//!   cannot drive the same pixels two ways at once.
//! - **Reduction happens at submit**, through [`encode`](crate::encode),
//!   using the derivation the contract settled on: the whole refreshed
//!   rect is diffused when a named region covers it, nothing is diffused
//!   when none were named, and otherwise diffusion runs inside the named
//!   regions only.
//!
//! Not modelled, deliberately: how ghosting looks, temperature, waveform
//! artifacts, and collision handling inside the controller. Those are the
//! facts a device has to supply, and a simulation of them would be a
//! guess dressed as a test.
//!
//! # What it is for
//!
//! Two things. A shell author drives their own code against it and
//! asserts on the glass and the fault log rather than on a device. And
//! it is the reference an EPDC backend is diffed against when hardware
//! arrives: the same conformance tests, run against this with the test's
//! clock and against the device with a real one.

use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;

use crate::{
    encode, Panel, PanelInfo, PanelRect, PixelFormat, Result, Source, Update, UpdateClass,
    UpdateToken, Wait,
};

/// How long each class takes, and how the controller aligns a region.
///
/// The durations only need to be ordered the way a real controller's are
/// — a monochrome update finishes well before a flash — for the driver's
/// behaviour to be exercised. The defaults are in the range an i.MX EPDC
/// reports, and are not a claim about any particular panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimConfig {
    /// Time to complete a [`UpdateClass::Monochrome`] update.
    pub monochrome: Duration,
    /// Time to complete a [`UpdateClass::Fast`] update.
    pub fast: Duration,
    /// Time to complete a [`UpdateClass::Quality`] update.
    pub quality: Duration,
    /// Time to complete a [`UpdateClass::Flash`] update.
    pub flash: Duration,
    /// The region is rounded outward to a multiple of this, in pixels,
    /// on both axes. Zero and one both mean no widening.
    pub alignment: u32,
}

impl SimConfig {
    /// How long an update of this class takes on this panel.
    pub const fn duration_for(&self, class: UpdateClass) -> Duration {
        match class {
            UpdateClass::Monochrome => self.monochrome,
            UpdateClass::Fast => self.fast,
            UpdateClass::Quality => self.quality,
            UpdateClass::Flash => self.flash,
        }
    }

    /// The grey levels the waveform for this class resolves.
    ///
    /// Two for a monochrome waveform, four for a fast one, sixteen for
    /// the full-range ones — the shape every EPDC shares, whatever the
    /// constants are called.
    pub const fn levels_for(class: UpdateClass) -> u8 {
        match class {
            UpdateClass::Monochrome => 2,
            UpdateClass::Fast => 4,
            UpdateClass::Quality | UpdateClass::Flash => 16,
        }
    }
}

impl Default for SimConfig {
    fn default() -> Self {
        SimConfig {
            monochrome: Duration::from_millis(120),
            fast: Duration::from_millis(260),
            quality: Duration::from_millis(450),
            flash: Duration::from_millis(800),
            alignment: 8,
        }
    }
}

/// A write that landed under an update the controller was still reading.
///
/// Recorded rather than refused: the trait puts the rule on the driver,
/// and a panel that enforced it too would hide a driver that does not.
/// A test asserts that [`SimulatedPanel::faults`] is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fault {
    /// The update that was live.
    pub token: UpdateToken,
    /// The region the driver asked it to refresh, which is the region
    /// the driver was responsible for keeping clear.
    pub live: PanelRect,
    /// The region that was written.
    pub written: PanelRect,
}

/// One submission, as the controller took it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Submitted {
    /// The token handed back.
    pub token: UpdateToken,
    /// The class, after the driver's policy had its say.
    pub class: UpdateClass,
    /// What was asked for.
    pub asked: PanelRect,
    /// What was refreshed: `asked`, widened to the alignment and clamped
    /// to the panel.
    pub refreshed: PanelRect,
    /// Where the consumer said diffusion could run.
    pub dither: Vec<PanelRect>,
    /// The clock when it was submitted.
    pub submitted_at: Duration,
    /// The clock when it reaches the glass.
    pub completes_at: Duration,
}

/// An update the controller has not finished with.
#[derive(Debug, Clone)]
pub struct InFlight {
    /// Its token.
    pub token: UpdateToken,
    /// The region it is reading — the refreshed one, not the asked one.
    pub rect: PanelRect,
    /// The region the driver asked for, which is all the driver knows.
    pub asked: PanelRect,
    /// When it reaches the glass.
    pub completes_at: Duration,
    /// The reduced pixels that will land there, `rect.w * rect.h * 4`.
    pixels: Vec<u8>,
}

/// A panel that keeps a clock, a staging buffer and a glass, and tells
/// you when something wrote where it should not have.
#[derive(Debug, Clone)]
pub struct SimulatedPanel {
    info: PanelInfo,
    config: SimConfig,
    now: Duration,
    next_token: u32,
    staging: Vec<u8>,
    glass: Vec<u8>,
    in_flight: Vec<InFlight>,
    /// Every submission, oldest first.
    pub updates: Vec<Submitted>,
    /// Every write under a live update, oldest first. Empty is the
    /// assertion.
    pub faults: Vec<Fault>,
}

impl SimulatedPanel {
    /// A panel of this size with the default timing.
    pub fn new(width: u32, height: u32) -> Self {
        Self::with_config(width, height, SimConfig::default())
    }

    /// A panel of this size with this timing.
    pub fn with_config(width: u32, height: u32, config: SimConfig) -> Self {
        // "Do not reduce; I will." The depth belongs to the update.
        let info = PanelInfo::new(width, height, PixelFormat::Rgba);
        SimulatedPanel {
            info,
            config,
            now: Duration::ZERO,
            next_token: 1,
            // White, which is what a panel shows after `INIT`.
            staging: vec![0xFF; info.buffer_len()],
            glass: vec![0xFF; info.buffer_len()],
            in_flight: Vec::new(),
            updates: Vec::new(),
            faults: Vec::new(),
        }
    }

    /// The timing this panel was built with.
    pub const fn config(&self) -> &SimConfig {
        &self.config
    }

    /// The clock.
    pub const fn now(&self) -> Duration {
        self.now
    }

    /// Move the clock forward, landing every update that completes on
    /// the way, in the order they complete.
    pub fn advance(&mut self, by: Duration) {
        self.now += by;
        loop {
            // The earliest completion still due; ties land in submission
            // order because the scan takes the first strict minimum.
            let Some(at) = self
                .in_flight
                .iter()
                .enumerate()
                .filter(|(_, u)| u.completes_at <= self.now)
                .min_by_key(|(_, u)| u.completes_at)
                .map(|(i, _)| i)
            else {
                return;
            };
            let done = self.in_flight.remove(at);
            self.land(&done);
        }
    }

    /// What the glass shows: RGBA, panel orientation, every channel of a
    /// pixel equal to its grey level.
    pub fn glass(&self) -> &[u8] {
        &self.glass
    }

    /// One pixel of the glass, or `None` off the panel.
    pub fn glass_pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.info.width || y >= self.info.height {
            return None;
        }
        let at = (y as usize * self.info.width as usize + x as usize) * 4;
        Some([
            self.glass[at],
            self.glass[at + 1],
            self.glass[at + 2],
            self.glass[at + 3],
        ])
    }

    /// How many distinct grey values the glass shows inside `rect`.
    ///
    /// Two after a monochrome update, up to sixteen after a quality one:
    /// the cheapest way to ask "did the right waveform run here".
    pub fn distinct_levels(&self, rect: PanelRect) -> usize {
        let rect = rect.clamped(self.info.width, self.info.height);
        let mut seen = [false; 256];
        for y in rect.y..rect.max_y() {
            for x in rect.x..rect.max_x() {
                let at = (y as usize * self.info.width as usize + x as usize) * 4;
                seen[self.glass[at] as usize] = true;
            }
        }
        seen.iter().filter(|&&s| s).count()
    }

    /// The updates the controller is still reading.
    pub fn in_flight(&self) -> &[InFlight] {
        &self.in_flight
    }

    /// `rect` widened outward to the alignment and clamped to the panel.
    fn widen(&self, rect: PanelRect) -> PanelRect {
        let a = self.config.alignment.max(1);
        let x = rect.x / a * a;
        let y = rect.y / a * a;
        let max_x = rect.max_x().div_ceil(a) * a;
        let max_y = rect.max_y().div_ceil(a) * a;
        PanelRect::new(x, y, max_x - x, max_y - y).clamped(self.info.width, self.info.height)
    }

    /// Copy a completed update onto the glass.
    fn land(&mut self, done: &InFlight) {
        let w = self.info.width as usize;
        let r = done.rect;
        for row in 0..r.h as usize {
            let from = row * r.w as usize * 4;
            let to = ((r.y as usize + row) * w + r.x as usize) * 4;
            let len = r.w as usize * 4;
            self.glass[to..to + len].copy_from_slice(&done.pixels[from..from + len]);
        }
    }

    /// Take `rect` out of staging and reduce it for `update`.
    fn reduce(&self, rect: PanelRect, update: Update<'_>) -> Vec<u8> {
        let w = self.info.width as usize;
        let mut out = Vec::with_capacity(rect.w as usize * rect.h as usize * 4);
        for y in rect.y..rect.max_y() {
            let at = (y as usize * w + rect.x as usize) * 4;
            out.extend_from_slice(&self.staging[at..at + rect.w as usize * 4]);
        }

        // The derivation from the contract: a hardware dither covers the
        // whole refreshed rect or nothing, so it applies exactly when a
        // named region covers that rect. Otherwise diffusion is software,
        // inside the regions, translated into the snapshot's coordinates.
        let local = PanelRect::full(rect.w, rect.h);
        let zones: Vec<PanelRect> = if update.dither_regions().iter().any(|z| z.contains(rect)) {
            vec![local]
        } else {
            update
                .dither_regions()
                .iter()
                .map(|z| z.intersection(rect))
                .filter(|z| !z.is_empty())
                .map(|z| PanelRect::new(z.x - rect.x, z.y - rect.y, z.w, z.h))
                .collect()
        };
        encode::quantize(
            &mut out,
            rect.w,
            rect.h,
            local,
            SimConfig::levels_for(update.class()),
            &zones,
        );
        out
    }
}

impl Panel for SimulatedPanel {
    fn info(&self) -> PanelInfo {
        self.info
    }

    fn blit(&mut self, src: Source<'_>, rect: PanelRect) -> Result<()> {
        src.require(rect)?;
        let rect = rect.clamped(self.info.width, self.info.height);
        // Two kinds of overlap, and they belong to different parties.
        //
        // Under the region the driver *asked* for: the driver's rule, and
        // the driver broke it. Recorded, then written anyway — the point
        // is to find out, not to stop the test at the first sign.
        //
        // Under only the margin this panel widened into: the driver could
        // not know, so the trait makes it the panel's problem, and the
        // panel does what a bus-attached one would — it cannot write
        // while the controller is busy, so it waits.
        let mut wait_until = None;
        for live in &self.in_flight {
            if !live.rect.intersects(rect) {
                continue;
            }
            if live.asked.intersects(rect) {
                self.faults.push(Fault {
                    token: live.token,
                    live: live.asked,
                    written: rect,
                });
            } else {
                wait_until = wait_until.max(Some(live.completes_at));
            }
        }
        if let Some(until) = wait_until {
            self.advance(until.saturating_sub(self.now));
        }
        let w = self.info.width as usize;
        for y in rect.y..rect.max_y() {
            let row = src.row(rect.x, y).expect("checked by require");
            let at = (y as usize * w + rect.x as usize) * 4;
            let len = rect.w as usize * 4;
            self.staging[at..at + len].copy_from_slice(&row[..len]);
        }
        Ok(())
    }

    fn submit(&mut self, rect: PanelRect, update: Update<'_>) -> Result<UpdateToken> {
        let asked = rect;
        let refreshed = self.widen(rect);
        let pixels = self.reduce(refreshed, update);

        // Queue behind whatever this overlaps: the controller cannot
        // drive the same pixels two ways at once.
        let after = self
            .in_flight
            .iter()
            .filter(|u| u.rect.intersects(refreshed))
            .map(|u| u.completes_at)
            .max()
            .unwrap_or(self.now)
            .max(self.now);
        let completes_at = after + self.config.duration_for(update.class());

        let token = UpdateToken::new(self.next_token);
        // Never zero: a controller's "no marker" is 0.
        self.next_token = self.next_token.wrapping_add(1).max(1);

        self.in_flight.push(InFlight {
            token,
            rect: refreshed,
            asked,
            completes_at,
            pixels,
        });
        self.updates.push(Submitted {
            token,
            class: update.class(),
            asked,
            refreshed,
            dither: update.dither_regions().to_vec(),
            submitted_at: self.now,
            completes_at,
        });
        Ok(token)
    }

    fn wait(&mut self, token: UpdateToken, timeout: Option<Duration>) -> Result<Wait> {
        let Some(completes_at) = self
            .in_flight
            .iter()
            .find(|u| u.token == token)
            .map(|u| u.completes_at)
        else {
            // Retired or never issued: the contract says that is not the
            // caller's problem.
            return Ok(Wait::Complete);
        };
        let remaining = completes_at.saturating_sub(self.now);
        match timeout {
            Some(t) if t < remaining => {
                self.advance(t);
                Ok(Wait::TimedOut)
            }
            _ => {
                self.advance(remaining);
                Ok(Wait::Complete)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A horizontal ramp, black at the left edge to white at the right.
    fn ramp(w: u32, h: u32) -> Vec<u8> {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let at = ((y * w + x) * 4) as usize;
                let v = (x * 255 / (w - 1).max(1)) as u8;
                rgba[at..at + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        rgba
    }

    fn flat(w: u32, h: u32, v: u8) -> Vec<u8> {
        let mut rgba = vec![v; (w * h * 4) as usize];
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        rgba
    }

    /// No widening, so rects are what the test says they are.
    fn exact() -> SimConfig {
        SimConfig {
            alignment: 1,
            ..SimConfig::default()
        }
    }

    #[test]
    fn pixels_reach_the_glass_only_when_the_update_completes() {
        let (w, h) = (16, 4);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let black = flat(w, h, 0);
        let src = Source::panel(&black, w, h).unwrap();
        let all = PanelRect::full(w, h);

        panel.blit(src, all).unwrap();
        let token = panel.submit(all, UpdateClass::Quality.into()).unwrap();
        // Staged, not shown.
        assert_eq!(panel.glass_pixel(0, 0), Some([255, 255, 255, 255]));

        panel.advance(ms(449));
        assert_eq!(panel.glass_pixel(0, 0), Some([255, 255, 255, 255]));
        assert_eq!(panel.in_flight().len(), 1);

        panel.advance(ms(1));
        assert_eq!(panel.glass_pixel(0, 0), Some([0, 0, 0, 255]));
        assert!(panel.in_flight().is_empty());
        assert_eq!(panel.wait(token, None).unwrap(), Wait::Complete);
    }

    #[test]
    fn a_bounded_wait_that_expires_leaves_the_update_live() {
        let (w, h) = (16, 4);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let black = flat(w, h, 0);
        let src = Source::panel(&black, w, h).unwrap();
        let all = PanelRect::full(w, h);
        panel.blit(src, all).unwrap();
        let token = panel.submit(all, UpdateClass::Flash.into()).unwrap();

        assert_eq!(panel.wait(token, Some(ms(100))).unwrap(), Wait::TimedOut);
        // Blocking is time passing: the deadline was spent.
        assert_eq!(panel.now(), ms(100));
        assert_eq!(panel.in_flight().len(), 1);

        // A zero timeout polls.
        assert_eq!(
            panel.wait(token, Some(Duration::ZERO)).unwrap(),
            Wait::TimedOut
        );
        assert_eq!(panel.now(), ms(100));

        assert_eq!(panel.wait(token, None).unwrap(), Wait::Complete);
        assert_eq!(panel.now(), ms(800));
        assert!(panel.in_flight().is_empty());
    }

    #[test]
    fn a_deadline_exactly_at_completion_is_met() {
        let (w, h) = (16, 4);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let black = flat(w, h, 0);
        let src = Source::panel(&black, w, h).unwrap();
        let all = PanelRect::full(w, h);
        panel.blit(src, all).unwrap();
        let token = panel.submit(all, UpdateClass::Monochrome.into()).unwrap();
        assert_eq!(panel.wait(token, Some(ms(120))).unwrap(), Wait::Complete);
    }

    #[test]
    fn waiting_on_a_retired_or_unknown_token_completes_at_once() {
        let mut panel = SimulatedPanel::with_config(16, 4, exact());
        assert_eq!(
            panel.wait(UpdateToken::new(99), None).unwrap(),
            Wait::Complete
        );
        assert_eq!(panel.now(), Duration::ZERO);
    }

    #[test]
    fn a_write_under_a_live_update_is_a_fault_and_elsewhere_is_not() {
        let (w, h) = (16, 8);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let black = flat(w, h, 0);
        let src = Source::panel(&black, w, h).unwrap();
        let top = PanelRect::new(0, 0, w, 4);
        let bottom = PanelRect::new(0, 4, w, 4);

        panel.blit(src, top).unwrap();
        let token = panel.submit(top, UpdateClass::Quality.into()).unwrap();

        panel.blit(src, bottom).unwrap();
        assert!(panel.faults.is_empty());

        panel.blit(src, top).unwrap();
        assert_eq!(
            panel.faults,
            vec![Fault {
                token,
                live: top,
                written: top
            }]
        );
    }

    #[test]
    fn the_refreshed_rect_is_widened_to_the_alignment_and_never_narrowed() {
        let (w, h) = (100, 50);
        let mut panel = SimulatedPanel::new(w, h);
        let black = flat(w, h, 0);
        let src = Source::panel(&black, w, h).unwrap();
        let asked = PanelRect::new(3, 5, 10, 10);
        panel.blit(src, asked).unwrap();
        panel.submit(asked, UpdateClass::Fast.into()).unwrap();

        let refreshed = panel.updates[0].refreshed;
        assert_eq!(refreshed, PanelRect::new(0, 0, 16, 16));
        assert!(refreshed.contains(asked));

        // And a rect at the far edge is widened up to the panel, not past.
        let edge = PanelRect::new(93, 45, 7, 5);
        panel.submit(edge, UpdateClass::Fast.into()).unwrap();
        assert_eq!(panel.updates[1].refreshed, PanelRect::new(88, 40, 12, 10));
    }

    #[test]
    fn a_write_into_the_widened_margin_makes_the_panel_wait_rather_than_fault() {
        // The driver only knows the rect it asked for. The panel is the
        // one that widened, so the trait makes the margin the panel's to
        // keep safe: it blocks, as a bus-attached panel would, and the
        // driver is not blamed.
        let (w, h) = (100, 50);
        let mut panel = SimulatedPanel::new(w, h);
        let black = flat(w, h, 0);
        let src = Source::panel(&black, w, h).unwrap();
        let asked = PanelRect::new(3, 5, 10, 10);
        panel.blit(src, asked).unwrap();
        panel.submit(asked, UpdateClass::Fast.into()).unwrap();
        assert_eq!(panel.updates[0].refreshed, PanelRect::new(0, 0, 16, 16));

        // Outside what was asked, inside what was refreshed.
        panel.blit(src, PanelRect::new(0, 0, 3, 3)).unwrap();
        assert!(panel.faults.is_empty());
        assert_eq!(panel.now(), ms(260), "the panel waited for its own update");
        assert!(panel.in_flight().is_empty());

        // Inside what was asked: that one is the driver's.
        panel.submit(asked, UpdateClass::Fast.into()).unwrap();
        panel.blit(src, PanelRect::new(5, 5, 2, 2)).unwrap();
        assert_eq!(panel.faults.len(), 1);
        assert_eq!(panel.faults[0].live, asked);
    }

    #[test]
    fn an_overlapping_update_queues_behind_the_one_in_flight() {
        let (w, h) = (16, 8);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let black = flat(w, h, 0);
        let src = Source::panel(&black, w, h).unwrap();
        let all = PanelRect::full(w, h);
        let top = PanelRect::new(0, 0, w, 4);
        let bottom = PanelRect::new(0, 4, w, 4);

        panel.blit(src, all).unwrap();
        panel.submit(top, UpdateClass::Quality.into()).unwrap(); // done at 450
        panel.advance(ms(100));
        panel
            .submit(bottom, UpdateClass::Monochrome.into())
            .unwrap(); // unrelated: 220
        panel.submit(top, UpdateClass::Monochrome.into()).unwrap(); // behind: 570

        let at: Vec<Duration> = panel.updates.iter().map(|u| u.completes_at).collect();
        assert_eq!(at, vec![ms(450), ms(220), ms(570)]);
    }

    #[test]
    fn updates_land_in_completion_order_not_submission_order() {
        let (w, h) = (16, 4);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let all = PanelRect::full(w, h);
        // A slow black update on the left half, then a quick white one on
        // the right half; both complete during one advance, white first.
        let black = flat(w, h, 0);
        let white = flat(w, h, 255);
        let left = PanelRect::new(0, 0, 8, h);
        let right = PanelRect::new(8, 0, 8, h);
        panel
            .blit(Source::panel(&black, w, h).unwrap(), all)
            .unwrap();
        panel.submit(left, UpdateClass::Flash.into()).unwrap();
        panel
            .blit(Source::panel(&white, w, h).unwrap(), right)
            .unwrap();
        panel.submit(right, UpdateClass::Monochrome.into()).unwrap();
        panel.advance(ms(1000));
        assert_eq!(panel.glass_pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(panel.glass_pixel(15, 0), Some([255, 255, 255, 255]));
        assert!(panel.in_flight().is_empty());
    }

    #[test]
    fn each_class_resolves_its_own_depth() {
        let (w, h) = (256, 2);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let page = ramp(w, h);
        let src = Source::panel(&page, w, h).unwrap();
        let all = PanelRect::full(w, h);
        for (class, levels) in [
            (UpdateClass::Monochrome, 2),
            (UpdateClass::Fast, 4),
            (UpdateClass::Quality, 16),
            (UpdateClass::Flash, 16),
        ] {
            panel.blit(src, all).unwrap();
            let token = panel.submit(all, class.into()).unwrap();
            assert_eq!(panel.wait(token, None).unwrap(), Wait::Complete);
            assert_eq!(panel.distinct_levels(all), levels, "{class:?}");
        }
    }

    #[test]
    fn a_region_covering_the_rect_diffuses_all_of_it() {
        let (w, h) = (32, 8);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let grey = flat(w, h, 128);
        let src = Source::panel(&grey, w, h).unwrap();
        let all = PanelRect::full(w, h);
        panel.blit(src, all).unwrap();
        let token = panel
            .submit(
                all,
                Update::new(UpdateClass::Monochrome).dithering_within(&[all]),
            )
            .unwrap();
        assert_eq!(panel.wait(token, None).unwrap(), Wait::Complete);
        // Mid grey at two levels: solid without diffusion, a stipple with.
        assert_eq!(panel.distinct_levels(all), 2);
    }

    #[test]
    fn no_regions_means_no_diffusion() {
        let (w, h) = (32, 8);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let grey = flat(w, h, 128);
        let src = Source::panel(&grey, w, h).unwrap();
        let all = PanelRect::full(w, h);
        panel.blit(src, all).unwrap();
        let token = panel.submit(all, UpdateClass::Monochrome.into()).unwrap();
        assert_eq!(panel.wait(token, None).unwrap(), Wait::Complete);
        assert_eq!(panel.distinct_levels(all), 1);
    }

    #[test]
    fn a_region_inside_the_rect_diffuses_only_itself() {
        let (w, h) = (32, 8);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let grey = flat(w, h, 128);
        let src = Source::panel(&grey, w, h).unwrap();
        let all = PanelRect::full(w, h);
        let photo = PanelRect::new(0, 0, 16, h);
        let text = PanelRect::new(16, 0, 16, h);
        panel.blit(src, all).unwrap();
        let token = panel
            .submit(
                all,
                Update::new(UpdateClass::Monochrome).dithering_within(&[photo]),
            )
            .unwrap();
        assert_eq!(panel.wait(token, None).unwrap(), Wait::Complete);
        assert_eq!(panel.distinct_levels(photo), 2);
        assert_eq!(panel.distinct_levels(text), 1);
    }

    #[test]
    fn a_region_is_translated_when_the_rect_does_not_start_at_the_origin() {
        let (w, h) = (32, 16);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let grey = flat(w, h, 128);
        let src = Source::panel(&grey, w, h).unwrap();
        let rect = PanelRect::new(8, 8, 16, 8);
        let photo = PanelRect::new(8, 8, 8, 8);
        let rest = PanelRect::new(16, 8, 8, 8);
        panel.blit(src, rect).unwrap();
        let token = panel
            .submit(
                rect,
                Update::new(UpdateClass::Monochrome).dithering_within(&[photo]),
            )
            .unwrap();
        assert_eq!(panel.wait(token, None).unwrap(), Wait::Complete);
        assert_eq!(panel.distinct_levels(photo), 2);
        assert_eq!(panel.distinct_levels(rest), 1);
        // And nothing outside the rect was touched.
        assert_eq!(panel.distinct_levels(PanelRect::new(0, 0, 8, 8)), 1);
        assert_eq!(panel.glass_pixel(0, 0), Some([255, 255, 255, 255]));
    }

    #[test]
    fn a_source_that_cannot_supply_the_region_is_refused_before_anything_is_staged() {
        let (w, h) = (16, 4);
        let mut panel = SimulatedPanel::with_config(w, h, exact());
        let strip = flat(w, 2, 0);
        let src = Source::at(&strip, PanelRect::new(0, 0, w, 2)).unwrap();
        assert!(panel.blit(src, PanelRect::full(w, h)).is_err());
        assert_eq!(panel.glass_pixel(0, 3), Some([255, 255, 255, 255]));
    }
}
