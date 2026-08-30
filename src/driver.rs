//! The rules a shell would otherwise have to remember, in one place that
//! can be tested without hardware.

use alloc::vec::Vec;
use core::time::Duration;

use crate::{
    Error, Panel, PanelInfo, PanelRect, RefreshPolicy, Result, Source, Update, UpdateClass,
    UpdateToken, Wait,
};

/// One update still reaching the glass.
///
/// A named pair rather than a tuple because it is public: a backend or a
/// shell reading `PanelDriver::live` should not have to remember which
/// element is which, and the shape can grow — a submission time, a class
/// — without breaking the callers that only wanted the region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LiveUpdate {
    /// The token the panel handed back.
    pub token: UpdateToken,
    /// The region the driver asked to be refreshed — the *requested* one,
    /// which is all a driver can know. A panel that widened is responsible
    /// for its own blit safety over what it actually took.
    pub rect: PanelRect,
}

/// Drives a [`Panel`] correctly.
///
/// There are three rules, and each is the kind of thing that works on a
/// desk and fails on a device.
///
/// **Do not write under a live update.** The controller is reading the
/// staged pixels while it drives the film. Blitting a region an in-flight
/// update covers tears, or leaves the panel showing a mixture of two
/// frames. The driver waits only when the regions actually overlap, so an
/// unrelated corner of the screen never pays for a slow refresh elsewhere.
///
/// This is enforced against the region that was *requested*, which is all
/// a driver can know: [`Panel::submit`] is free to refresh more than it
/// was asked to, and a panel that widens is responsible for its own
/// safety, as that contract says.
///
/// **Clean up after fast updates.** A monochrome update leaves its region
/// at two levels, and e-ink holds that until something disturbs it — so
/// the selected lines stay chunky after the finger lifts. The driver
/// cannot see that moment: it has no way to tell a mid-drag selection from
/// the last one. Only the shell knows the pointer came up, so it calls
/// [`settle`](PanelDriver::settle) and the driver repaints what it left
/// provisional.
///
/// **Ration the flash.** Delegated to [`RefreshPolicy`].
///
/// # The set of live updates is a set
///
/// Tracking one outstanding update is the natural first implementation and
/// it is wrong. Submit over the top band, then over the bottom band —
/// correctly without waiting, the regions are disjoint — and the single
/// slot now holds the second, having silently forgotten the first. A third
/// update overlapping the first then blits under a refresh that is still
/// running, which is precisely the tearing the rule exists to prevent, and
/// a `flush` at teardown waits for one update while another is still
/// reaching the glass.
///
/// Neither shows up against a backend whose `submit` is a no-op, which is
/// what a plain framebuffer is, so the bug survives a green test suite and
/// bites on the first real controller. Hence a vector — short, because the
/// set is bounded by what the hardware will queue, which is a handful —
/// and `a_third_update_does_not_write_under_the_first` and
/// `flush_waits_for_every_live_update` below.
#[derive(Debug)]
pub struct PanelDriver<P: Panel> {
    panel: P,
    policy: RefreshPolicy,
    /// Every update still reaching the glass, oldest first.
    in_flight: Vec<LiveUpdate>,
    /// Region left at reduced fidelity, owed a full-fidelity pass.
    provisional: Option<PanelRect>,
}

impl<P: Panel> PanelDriver<P> {
    /// Drive `panel` with the default ghosting cadence.
    pub fn new(panel: P) -> Self {
        PanelDriver::with_policy(panel, RefreshPolicy::default())
    }

    /// Drive `panel` with a chosen ghosting cadence.
    pub fn with_policy(panel: P, policy: RefreshPolicy) -> Self {
        PanelDriver {
            panel,
            policy,
            in_flight: Vec::new(),
            provisional: None,
        }
    }

    /// Geometry and capabilities of the panel underneath.
    pub fn info(&self) -> PanelInfo {
        self.panel.info()
    }

    /// The panel underneath, for a backend's own diagnostics.
    pub fn panel(&self) -> &P {
        &self.panel
    }

    /// Reach the panel directly.
    ///
    /// Submitting through this goes behind the driver's back: the update
    /// it queues is not in the live set, so the next `present` over that
    /// region will not wait for it. Reading — `info`, a backend's own
    /// diagnostics — is what this is for.
    pub fn panel_mut(&mut self) -> &mut P {
        &mut self.panel
    }

    /// Give the panel back, discarding the driver's state. Anything
    /// still in flight stays in flight; call [`flush`](PanelDriver::flush)
    /// first.
    pub fn into_panel(self) -> P {
        self.panel
    }

    /// The ghosting cadence in force.
    pub fn policy(&self) -> &RefreshPolicy {
        &self.policy
    }

    /// Change the ghosting cadence, or reset its debt after a flash the
    /// shell decided on itself.
    pub fn policy_mut(&mut self) -> &mut RefreshPolicy {
        &mut self.policy
    }

    /// The region owed a full-fidelity repaint, if any.
    pub fn provisional(&self) -> Option<PanelRect> {
        self.provisional
    }

    /// Every update still reaching the glass, oldest first.
    pub fn live(&self) -> &[LiveUpdate] {
        &self.in_flight
    }

    /// Put pixels on the panel.
    ///
    /// `src` covers at least the region that will be refreshed, already
    /// quantized to [`PanelInfo::format`] and in panel orientation.
    /// `damage` is the region that changed, with `None` meaning the whole
    /// panel. `update` is the class, and optionally the regions error
    /// diffusion may run inside — a bare [`UpdateClass`] converts, so the
    /// common call passes one.
    ///
    /// Returns the token of the update it submitted, or `None` when there
    /// was nothing to do.
    pub fn present<'u>(
        &mut self,
        src: Source<'_>,
        damage: Option<PanelRect>,
        update: impl Into<Update<'u>>,
    ) -> Result<Option<UpdateToken>> {
        let update = update.into();
        let full = self.panel.info().bounds();

        // Order matters here, and it is the whole of three bugs.
        //
        // The region is settled, the caller's mistakes are found, and only
        // then does the policy get to have an opinion — because
        // `RefreshPolicy::resolve` *mutates*. Asking it first means a
        // present that turns out to do nothing still accrues ghost debt,
        // so a page turn later flashes early on debt no update incurred;
        // and it means an escalation can rewrite an empty rect into a
        // whole-panel flash, which is a black-white blink for a shell
        // whose damage arithmetic came out empty and was promised `None`.
        //
        // Clamping first is not narrowing in the sense rule 4 forbids.
        // Pixels off the panel do not exist, so a region that runs past
        // the edge is not a region a refresh could ever cover; leaving it
        // unclamped puts an off-panel rect into an ioctl.
        let asked = damage.unwrap_or(full).intersection(full);

        // A flash covers the whole panel by definition, so its damage rect
        // says nothing — including when it says nothing at all. Everywhere
        // else here an explicit flash ignores `damage`; letting the
        // emptiness gate be the one place it still decided something would
        // mean a shell that said "clear the ghosting" and passed its
        // damage rect along silently got neither.
        //
        // A consumer that asks for a flash must therefore have supplied
        // the whole panel. Checking that now, rather than after the policy
        // has run, keeps a caller bug from leaving the ghost debt looking
        // paid.
        if update.class() == UpdateClass::Flash {
            src.require(full)?;
        } else {
            if asked.is_empty() {
                return Ok(None);
            }
            src.require(asked)?;
        }

        // `RefreshPolicy::resolve` mutates, and from here on the call can
        // still fail inside the panel. Keeping a copy is what lets the
        // cadence be put back exactly if it does.
        let before = self.policy;
        let class = self.resolve_for(update.class(), src);
        let rect = if class == UpdateClass::Flash {
            full
        } else {
            asked
        };
        // Only reachable on a panel with no pixels, which nothing can show
        // anything on.
        if rect.is_empty() {
            self.policy = before;
            return Ok(None);
        }
        self.submit_or_rewind(src, rect, update.with_class(class), before)
            .map(Some)
    }

    /// Repaint whatever a fast update left degraded — what a shell calls
    /// when a gesture ends.
    ///
    /// `Ok(None)` when nothing is owed, which is the common case and
    /// deliberately cheap: a shell can call this on every idle tick.
    ///
    /// `dither` names the regions diffusion may run inside for the
    /// repaint, exactly as in [`present`](PanelDriver::present). `&[]` is
    /// the right answer for a page of text, which is what a selection
    /// drag is usually over.
    pub fn settle(&mut self, src: Source<'_>, dither: &[PanelRect]) -> Result<Option<UpdateToken>> {
        let Some(owed) = self.provisional else {
            return Ok(None);
        };
        // Same ordering rule as `present`: nothing touches the policy
        // until the call is known to be one that can go through.
        src.require(owed)?;
        // Through the policy like any other update: the cleanup pass is a
        // real change to the panel and may itself come due for a flash.
        let before = self.policy;
        let class = self.resolve_for(UpdateClass::Quality, src);
        let rect = if class == UpdateClass::Flash {
            self.panel.info().bounds()
        } else {
            owed
        };
        let update = Update::new(class).dithering_within(dither);
        self.submit_or_rewind(src, rect, update, before).map(Some)
    }

    /// Block until every outstanding update has reached the glass. A shell
    /// owes this before it tears the panel down, and nowhere else.
    pub fn flush(&mut self) -> Result<()> {
        while let Some(live) = self.in_flight.first().copied() {
            self.wait_for(live.token, None)?;
            self.in_flight.remove(0);
        }
        Ok(())
    }

    /// [`flush`](PanelDriver::flush) with a deadline, for a shell that
    /// would rather come back than hang on a wedged controller.
    ///
    /// `timeout` applies to each remaining update in turn, not to the
    /// flush as a whole — the trait's deadline is per wait, and pretending
    /// otherwise would need a clock this crate does not have. Returns
    /// [`Wait::TimedOut`] on the first update that does not finish;
    /// everything before it has been retired, and the rest are still live.
    pub fn flush_within(&mut self, timeout: Duration) -> Result<Wait> {
        while let Some(live) = self.in_flight.first().copied() {
            if self.panel.wait(live.token, Some(timeout))? == Wait::TimedOut {
                return Ok(Wait::TimedOut);
            }
            self.in_flight.remove(0);
        }
        Ok(Wait::Complete)
    }

    /// What the policy says, minus a flash the caller cannot actually
    /// supply the pixels for.
    ///
    /// A scheduled flash repaints the whole panel, and a consumer holding
    /// only the strip that changed does not have the rest of it. Failing
    /// there would mean a partial-buffer consumer hitting an error on a
    /// cadence it never asked about, which is a poor reward for taking the
    /// cheaper convention. So the escalation is deferred instead: the debt
    /// stays owed and the next update with a whole panel behind it pays it.
    ///
    /// A flash the *consumer* asked for is not deferred. It supplies what
    /// it asked for or it gets an error naming the region it was short of,
    /// which is the correct answer to "flash the screen from this strip".
    fn resolve_for(&mut self, requested: UpdateClass, src: Source<'_>) -> UpdateClass {
        let full = self.panel.info().bounds();
        let class = self.policy.resolve(requested);
        if class == UpdateClass::Flash && requested != UpdateClass::Flash && !src.covers(full) {
            self.policy.defer_flash();
            requested
        } else {
            class
        }
    }

    /// Wait on one token, insisting that an unbounded wait completes.
    fn wait_for(&mut self, token: UpdateToken, timeout: Option<Duration>) -> Result<()> {
        match self.panel.wait(token, timeout)? {
            Wait::Complete => Ok(()),
            // Unreachable against a backend that honours the contract, and
            // the only safe response if one does not: proceeding would
            // blit under a refresh that is still running.
            Wait::TimedOut => Err(Error::WaitStalled { token }),
        }
    }

    /// Retire every live update that overlaps `rect`, waiting for each.
    ///
    /// Only an overlap forces a wait; a change elsewhere on the page
    /// proceeds while the previous refresh is still running.
    fn clear_the_way(&mut self, rect: PanelRect) -> Result<()> {
        let mut at = 0;
        while at < self.in_flight.len() {
            let live = self.in_flight[at];
            if live.rect.intersects(rect) {
                self.wait_for(live.token, None)?;
                self.in_flight.remove(at);
            } else {
                at += 1;
            }
        }
        Ok(())
    }

    /// [`submit_region`](PanelDriver::submit_region), putting the ghost
    /// cadence back if it fails.
    ///
    /// `resolve_for` has to run before the region is settled, because the
    /// region depends on what it decides — but a submission that never
    /// reached the glass did not ghost anything, so the panel must not be
    /// charged for it. The alternative is a cadence that walks forward on
    /// a failing backend and spends a flash no update paid for.
    /// `RefreshPolicy` is `Copy`, which makes the undo exact rather than a
    /// reconstruction.
    fn submit_or_rewind(
        &mut self,
        src: Source<'_>,
        rect: PanelRect,
        update: Update<'_>,
        before: RefreshPolicy,
    ) -> Result<UpdateToken> {
        let outcome = self.submit_region(src, rect, update);
        if outcome.is_err() {
            self.policy = before;
        }
        outcome
    }

    fn submit_region(
        &mut self,
        src: Source<'_>,
        rect: PanelRect,
        update: Update<'_>,
    ) -> Result<UpdateToken> {
        // Check before waiting. A source that cannot supply the region is
        // a caller bug, and finding it out after having blocked on a
        // refresh makes it look like a device problem.
        src.require(rect)?;
        self.clear_the_way(rect)?;
        self.panel.blit(src, rect)?;
        let token = self.panel.submit(rect, update)?;
        self.in_flight.push(LiveUpdate { token, rect });

        self.provisional = if update.class().is_provisional() {
            // Successive fast updates chase a moving gesture; the debt is
            // everything they have touched.
            Some(match self.provisional {
                Some(owed) => owed.union(rect),
                None => rect,
            })
        } else {
            // A full-fidelity update pays off only what it actually
            // covered.
            self.provisional.filter(|owed| !rect.contains(*owed))
        };
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PixelFormat, RecordingPanel};
    use alloc::vec;
    use alloc::vec::Vec;

    fn info(width: u32, height: u32) -> PanelInfo {
        PanelInfo::new(
            width,
            height,
            PixelFormat::Grey {
                levels: 16,
                dither: false,
            },
        )
    }

    fn driver(flash_every: u32) -> (PanelDriver<RecordingPanel>, Vec<u8>) {
        let buffer = vec![0u8; 100 * 200 * 4];
        (
            PanelDriver::with_policy(
                RecordingPanel::new(info(100, 200)),
                RefreshPolicy::new(flash_every),
            ),
            buffer,
        )
    }

    fn src(buffer: &[u8]) -> Source<'_> {
        Source::panel(buffer, 100, 200).unwrap()
    }

    // ---- The rule against writing under a live update ----

    #[test]
    fn an_overlapping_update_waits_for_the_one_in_flight() {
        let (mut driver, buffer) = driver(60);
        let first = driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap()
            .unwrap();
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 10, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap();
        assert_eq!(
            driver.panel().waited(),
            vec![first],
            "writing under a live update tears the panel"
        );
    }

    #[test]
    fn an_unrelated_region_does_not_wait() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap();
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 100, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap();
        assert!(
            driver.panel().waits.is_empty(),
            "a far corner paid for a refresh it did not overlap"
        );
    }

    #[test]
    fn a_third_update_does_not_write_under_the_first() {
        // The regression a single in-flight slot cannot survive. A covers
        // the top band; B covers the bottom and correctly waits for
        // nothing, since they are disjoint. With one slot, B has now
        // displaced A and the driver has forgotten both A's token and A's
        // region — so C, which overlaps A, blits under a refresh that is
        // still running.
        //
        // Invisible against a framebuffer, whose submit is a no-op and
        // where nothing is ever really in flight. Fatal on the first EPDC.
        let (mut driver, buffer) = driver(60);
        let top = PanelRect::new(0, 0, 100, 20);
        let bottom = PanelRect::new(0, 150, 100, 20);

        let a = driver
            .present(src(&buffer), Some(top), UpdateClass::Fast)
            .unwrap()
            .unwrap();
        let b = driver
            .present(src(&buffer), Some(bottom), UpdateClass::Fast)
            .unwrap()
            .unwrap();
        assert!(
            driver.panel().waits.is_empty(),
            "disjoint regions should not have waited"
        );
        assert_eq!(driver.live().len(), 2, "the first update was forgotten");

        let c = driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 10, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            driver.panel().waited(),
            vec![a],
            "the third update wrote under the first"
        );
        // A is retired; B, which C did not overlap, was neither waited
        // for nor forgotten.
        assert_eq!(
            driver
                .live()
                .iter()
                .map(|live| live.token)
                .collect::<Vec<_>>(),
            vec![b, c]
        );
    }

    #[test]
    fn flush_waits_for_every_live_update() {
        // The other half of the same defect: with one slot, a flush at
        // teardown waits for the most recent update and leaves the earlier
        // one reaching the glass while the process exits.
        let (mut driver, buffer) = driver(60);
        let a = driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap()
            .unwrap();
        let b = driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 150, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap()
            .unwrap();

        driver.flush().unwrap();
        assert_eq!(
            driver.panel().waited(),
            vec![a, b],
            "an update outlived the flush"
        );
        assert!(driver.live().is_empty());
        // Nothing outstanding: a second flush is free.
        driver.flush().unwrap();
        assert_eq!(driver.panel().waits.len(), 2);
    }

    #[test]
    fn a_completed_update_stops_being_waited_for() {
        // Retiring on wait is what keeps the vector short. Without it the
        // set grows for the life of the process and every update pays to
        // scan it.
        let (mut driver, buffer) = driver(60);
        let rect = PanelRect::new(0, 0, 100, 20);
        for _ in 0..5 {
            driver
                .present(src(&buffer), Some(rect), UpdateClass::Fast)
                .unwrap();
        }
        assert_eq!(driver.live().len(), 1);
        assert_eq!(driver.panel().waits.len(), 4);
    }

    #[test]
    fn the_driver_blocks_rather_than_polling_when_it_has_to_wait() {
        // A deadline exists on the trait for a shell that wants one. The
        // driver's own waits are the ones that must not proceed early,
        // so they are unbounded.
        let (mut driver, buffer) = driver(60);
        let rect = PanelRect::new(0, 0, 100, 20);
        driver
            .present(src(&buffer), Some(rect), UpdateClass::Fast)
            .unwrap();
        driver
            .present(src(&buffer), Some(rect), UpdateClass::Fast)
            .unwrap();
        assert_eq!(driver.panel().waits[0].1, None);
    }

    // ---- Cleaning up after fast updates ----

    #[test]
    fn a_drag_issues_fast_updates_and_never_flashes() {
        let (mut driver, buffer) = driver(6);
        for y in 0..40 {
            let rect = PanelRect::new(0, y, 100, 20);
            driver
                .present(src(&buffer), Some(rect), UpdateClass::Monochrome)
                .unwrap();
        }
        let panel = driver.panel();
        assert_eq!(panel.count(UpdateClass::Monochrome), 40);
        assert_eq!(panel.count(UpdateClass::Flash), 0, "flashed under a drag");
    }

    #[test]
    fn settling_after_a_drag_repaints_what_it_degraded() {
        let (mut driver, buffer) = driver(60);
        for y in [0, 20, 40] {
            driver
                .present(
                    src(&buffer),
                    Some(PanelRect::new(0, y, 100, 20)),
                    UpdateClass::Monochrome,
                )
                .unwrap();
        }
        // The debt is everything the gesture touched, not just its last step.
        assert_eq!(driver.provisional(), Some(PanelRect::new(0, 0, 100, 60)));

        driver
            .settle(src(&buffer), &[])
            .unwrap()
            .expect("a cleanup update");
        assert_eq!(driver.provisional(), None);
        let last = driver.panel().updates.last().unwrap();
        assert_eq!(last.class, UpdateClass::Quality);
        assert_eq!(last.rect, PanelRect::new(0, 0, 100, 60));
    }

    #[test]
    fn settling_with_nothing_owed_does_nothing() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(src(&buffer), None, UpdateClass::Quality)
            .unwrap()
            .expect("the page itself");
        let before = driver.panel().updates.len();
        // Cheap enough for a shell to call on every idle tick.
        for _ in 0..10 {
            assert!(driver.settle(src(&buffer), &[]).unwrap().is_none());
        }
        assert_eq!(driver.panel().updates.len(), before);
    }

    #[test]
    fn a_covering_repaint_pays_the_debt_without_settling() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Monochrome,
            )
            .unwrap();
        assert!(driver.provisional().is_some());
        // A page turn repaints the lines anyway; the cleanup is already done.
        driver
            .present(src(&buffer), None, UpdateClass::Quality)
            .unwrap();
        assert_eq!(driver.provisional(), None);
        assert!(driver.settle(src(&buffer), &[]).unwrap().is_none());
    }

    #[test]
    fn a_partial_repaint_leaves_the_rest_owed() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 0, 100, 60)),
                UpdateClass::Monochrome,
            )
            .unwrap();
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Quality,
            )
            .unwrap();
        assert_eq!(
            driver.provisional(),
            Some(PanelRect::new(0, 0, 100, 60)),
            "a repaint that covered part of the region cleared all of it"
        );
    }

    // ---- Rationing the flash ----

    #[test]
    fn a_flash_covers_the_whole_panel() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 0, 10, 10)),
                UpdateClass::Flash,
            )
            .unwrap();
        let first = &driver.panel().updates[0];
        assert_eq!(first.class, UpdateClass::Flash);
        assert_eq!(
            first.rect,
            PanelRect::full(100, 200),
            "a flash of one corner spends the blink without clearing the screen"
        );
    }

    #[test]
    fn the_flash_cadence_survives_the_driver() {
        let (mut driver, buffer) = driver(3);
        for _ in 0..6 {
            driver
                .present(src(&buffer), None, UpdateClass::Quality)
                .unwrap();
        }
        assert_eq!(driver.panel().count(UpdateClass::Flash), 2);
        assert_eq!(driver.panel().count(UpdateClass::Quality), 4);
    }

    #[test]
    fn escalating_to_a_flash_keeps_the_diffusion_hints() {
        // The escalation is the driver's decision, not the consumer's, so
        // it is the driver that would drop what the consumer said about
        // where its photographs are — and a flash is exactly the update
        // where losing them is most visible.
        let (mut driver, buffer) = driver(1);
        let images = [PanelRect::new(10, 10, 40, 40)];
        driver
            .present(
                src(&buffer),
                None,
                Update::new(UpdateClass::Quality).dithering_within(&images),
            )
            .unwrap();
        let first = &driver.panel().updates[0];
        assert_eq!(first.class, UpdateClass::Flash);
        assert_eq!(first.dither, images);
    }

    // ---- Nothing to do ----

    #[test]
    fn nothing_to_show_touches_nothing() {
        let (mut driver, buffer) = driver(60);
        // Damage that came out empty — a change that landed off-panel.
        assert!(driver
            .present(src(&buffer), Some(PanelRect::default()), UpdateClass::Fast)
            .unwrap()
            .is_none());
        assert!(driver.panel().updates.is_empty());
        assert!(driver.panel().blits.is_empty());
    }

    #[test]
    fn an_empty_damage_rect_cannot_flash_the_panel() {
        // The escalation used to run before the emptiness check and then
        // overwrite the rect with the whole panel, so a shell whose damage
        // arithmetic came out empty got a black-white blink — and
        // `present`, which promises `None` when there is nothing to do,
        // handed back a token instead. `nothing_to_show_touches_nothing`
        // could not see it: `Fast` never escalates.
        let (mut driver, buffer) = driver(1);
        assert!(driver
            .present(
                src(&buffer),
                Some(PanelRect::default()),
                UpdateClass::Quality
            )
            .unwrap()
            .is_none());
        assert!(driver.panel().updates.is_empty(), "flashed for no change");
        assert!(driver.panel().blits.is_empty());
    }

    #[test]
    fn an_update_that_does_nothing_incurs_no_ghost_debt() {
        // `RefreshPolicy::resolve` mutates, so asking it before knowing
        // whether there is anything to show charges the panel for updates
        // that never happened — and the next real page turn then flashes
        // early, on debt nothing incurred.
        let (mut driver, buffer) = driver(3);
        for _ in 0..5 {
            driver
                .present(
                    src(&buffer),
                    Some(PanelRect::default()),
                    UpdateClass::Quality,
                )
                .unwrap();
        }
        assert_eq!(driver.policy().since_flash(), 0);
    }

    /// A panel that refuses everything, for the half of "failed" that is
    /// not the caller's fault.
    #[derive(Debug)]
    struct Broken;

    impl Panel for Broken {
        fn info(&self) -> PanelInfo {
            info(100, 200)
        }
        fn blit(&mut self, _src: Source<'_>, _rect: PanelRect) -> Result<()> {
            Err(Error::UnsupportedGeometry {
                width: 0,
                height: 0,
            })
        }
        fn submit(&mut self, _rect: PanelRect, _update: Update<'_>) -> Result<UpdateToken> {
            unreachable!("a blit that failed should not have been followed by a submit")
        }
        fn wait(&mut self, _token: UpdateToken, _timeout: Option<Duration>) -> Result<Wait> {
            Ok(Wait::Complete)
        }
    }

    /// A panel that works for `good` blits and then refuses.
    ///
    /// Incurring a debt takes an update that *landed*, so a panel that
    /// fails from the first call cannot reach the state a failed cleanup
    /// pass is about — it can only reach `settle`'s early return, where
    /// nothing is owed and nothing is tested.
    #[derive(Debug)]
    struct FailsAfter {
        good: u32,
        next_token: u32,
    }

    impl FailsAfter {
        fn new(good: u32) -> Self {
            FailsAfter {
                good,
                next_token: 1,
            }
        }
    }

    impl Panel for FailsAfter {
        fn info(&self) -> PanelInfo {
            info(100, 200)
        }
        fn blit(&mut self, _src: Source<'_>, _rect: PanelRect) -> Result<()> {
            match self.good.checked_sub(1) {
                Some(left) => {
                    self.good = left;
                    Ok(())
                }
                None => Err(Error::UnsupportedGeometry {
                    width: 0,
                    height: 0,
                }),
            }
        }
        fn submit(&mut self, _rect: PanelRect, _update: Update<'_>) -> Result<UpdateToken> {
            let token = UpdateToken::new(self.next_token);
            self.next_token += 1;
            Ok(token)
        }
        fn wait(&mut self, _token: UpdateToken, _timeout: Option<Duration>) -> Result<Wait> {
            Ok(Wait::Complete)
        }
    }

    #[test]
    fn an_update_that_failed_incurs_no_ghost_debt() {
        // The pixels never reached the panel, so nothing was ghosted —
        // and that is as true of a backend that refused as of a caller
        // that handed over a buffer too small, so both belong here. The
        // second is the one that needs the policy rewound: the region
        // cannot be settled until the policy has spoken, and the panel can
        // still fail afterwards.
        let (mut driver, buffer) = driver(3);
        let corner = &buffer[..10 * 10 * 4];
        for _ in 0..5 {
            let partial = Source::at(corner, PanelRect::new(0, 0, 10, 10)).unwrap();
            assert!(driver
                .present(
                    partial,
                    Some(PanelRect::new(0, 0, 100, 20)),
                    UpdateClass::Fast
                )
                .is_err());
        }
        assert_eq!(
            driver.policy().since_flash(),
            0,
            "a caller error was charged"
        );

        let mut driver = PanelDriver::with_policy(Broken, RefreshPolicy::new(3));
        let page = vec![0u8; 100 * 200 * 4];
        for _ in 0..5 {
            let src = Source::panel(&page, 100, 200).unwrap();
            assert!(driver.present(src, None, UpdateClass::Quality).is_err());
        }
        assert_eq!(
            driver.policy().since_flash(),
            0,
            "a backend error walked the cadence towards a flash that never reached glass"
        );
    }

    #[test]
    fn a_failed_settle_leaves_the_debt_where_it_was() {
        // A gesture lands, leaving its region provisional and one update
        // of ghost debt behind it. The cleanup pass then fails inside the
        // panel — so the region is still owed a repaint and the panel was
        // still only ghosted once, because the pass that would have made
        // it twice never reached the glass.
        let mut driver = PanelDriver::with_policy(FailsAfter::new(1), RefreshPolicy::new(3));
        let page = vec![0u8; 100 * 200 * 4];
        let source = || Source::panel(&page, 100, 200).unwrap();
        let owed = PanelRect::new(0, 0, 100, 20);

        driver
            .present(source(), Some(owed), UpdateClass::Monochrome)
            .unwrap()
            .expect("the gesture itself");
        assert_eq!(driver.provisional(), Some(owed));
        assert_eq!(driver.policy().since_flash(), 1);

        assert!(
            driver.settle(source(), &[]).is_err(),
            "the settle succeeded"
        );
        assert_eq!(
            driver.provisional(),
            Some(owed),
            "a cleanup that never landed stopped the region being owed"
        );
        assert_eq!(
            driver.policy().since_flash(),
            1,
            "a cleanup that never landed was charged to the panel anyway"
        );
    }

    #[test]
    fn a_flash_that_could_not_happen_does_not_pay_the_debt() {
        let (mut driver, buffer) = driver(3);
        for _ in 0..2 {
            driver
                .present(src(&buffer), None, UpdateClass::Quality)
                .unwrap();
        }
        let corner = &buffer[..10 * 10 * 4];
        let partial = Source::at(corner, PanelRect::new(0, 0, 10, 10)).unwrap();
        assert!(driver
            .present(
                partial,
                Some(PanelRect::new(0, 0, 10, 10)),
                UpdateClass::Flash
            )
            .is_err());
        assert_eq!(
            driver.policy().since_flash(),
            2,
            "a flash that never happened cleaned the panel"
        );
    }

    #[test]
    fn an_explicit_flash_ignores_its_damage_rect_entirely() {
        // A flash covers the whole panel by definition, so `damage` has no
        // say in what it refreshes — and an empty or off-panel rect is
        // still no say rather than a veto. A shell that asks to clear the
        // ghosting while passing its damage rect along should not silently
        // get nothing.
        for damage in [
            None,
            Some(PanelRect::default()),
            Some(PanelRect::new(0, 0, 10, 10)),
            Some(PanelRect::new(900, 900, 4, 4)),
        ] {
            let (mut driver, buffer) = driver(60);
            driver
                .present(src(&buffer), damage, UpdateClass::Flash)
                .unwrap()
                .unwrap_or_else(|| panic!("{damage:?} produced no update"));
            let first = &driver.panel().updates[0];
            assert_eq!(first.class, UpdateClass::Flash, "{damage:?}");
            assert_eq!(first.rect, PanelRect::full(100, 200), "{damage:?}");
        }
    }

    #[test]
    fn damage_past_the_edge_is_clamped_to_the_panel() {
        // Pixels off the panel do not exist, so trimming to the edge is
        // not the narrowing rule 4 forbids — and leaving it puts an
        // off-panel region into an ioctl.
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(80, 180, 500, 500)),
                UpdateClass::Fast,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            driver.panel().rects(),
            vec![PanelRect::new(80, 180, 20, 20)]
        );
        assert_eq!(driver.panel().blits, vec![PanelRect::new(80, 180, 20, 20)]);

        // Entirely off-panel is nothing to do, not an error.
        assert!(driver
            .present(
                src(&buffer),
                Some(PanelRect::new(500, 500, 10, 10)),
                UpdateClass::Fast,
            )
            .unwrap()
            .is_none());
        assert_eq!(driver.panel().updates.len(), 1);
    }

    #[test]
    fn unstated_damage_means_the_whole_panel() {
        let (mut driver, buffer) = driver(60);
        driver
            .present(src(&buffer), None, UpdateClass::Quality)
            .unwrap()
            .unwrap();
        assert_eq!(driver.panel().updates[0].rect, PanelRect::full(100, 200));
    }

    #[test]
    fn a_source_short_of_the_region_fails_before_anything_blocks() {
        // A caller bug, and it should read as one: finding out after
        // having blocked on a refresh makes it look like a device fault.
        let (mut driver, buffer) = driver(60);
        driver
            .present(
                src(&buffer),
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap();
        let corner = &buffer[..10 * 10 * 4];
        let partial = Source::at(corner, PanelRect::new(0, 0, 10, 10)).unwrap();
        let err = driver
            .present(
                partial,
                Some(PanelRect::new(0, 0, 100, 20)),
                UpdateClass::Fast,
            )
            .unwrap_err();
        assert!(matches!(err, Error::NotCovered { .. }), "{err:?}");
        assert!(driver.panel().waits.is_empty(), "blocked before checking");
        assert_eq!(driver.panel().blits.len(), 1);
    }

    #[test]
    fn a_partial_buffer_defers_a_scheduled_flash_rather_than_failing() {
        // A consumer that hands over only the strip it repainted cannot
        // supply a whole-panel flash, and should not discover that as an
        // error on a cadence it never asked about. The debt is deferred,
        // not forgotten: the next full-panel update pays it.
        let mut driver =
            PanelDriver::with_policy(RecordingPanel::new(info(100, 200)), RefreshPolicy::new(1));
        let strip = vec![0u8; 100 * 20 * 4];
        let rect = PanelRect::new(0, 0, 100, 20);
        driver
            .present(
                Source::at(&strip, rect).unwrap(),
                Some(rect),
                UpdateClass::Quality,
            )
            .unwrap()
            .unwrap();
        assert_eq!(driver.panel().classes(), vec![UpdateClass::Quality]);
        assert_eq!(driver.panel().rects(), vec![rect]);

        let page = vec![0u8; 100 * 200 * 4];
        driver
            .present(src(&page), None, UpdateClass::Quality)
            .unwrap();
        assert_eq!(
            driver.panel().updates[1].class,
            UpdateClass::Flash,
            "the deferred flash was forgotten instead of postponed"
        );
    }

    #[test]
    fn a_flash_the_consumer_asked_for_is_not_deferred() {
        // Asking for a flash and supplying a strip is a caller bug, and
        // the error names the region it was short of.
        let mut driver =
            PanelDriver::with_policy(RecordingPanel::new(info(100, 200)), RefreshPolicy::never());
        let strip = vec![0u8; 100 * 20 * 4];
        let rect = PanelRect::new(0, 0, 100, 20);
        let err = driver
            .present(
                Source::at(&strip, rect).unwrap(),
                Some(rect),
                UpdateClass::Flash,
            )
            .unwrap_err();
        assert!(matches!(err, Error::NotCovered { .. }), "{err:?}");
    }

    #[test]
    fn a_sub_buffer_covering_only_the_damage_is_enough() {
        // The convention the whole `Source` type exists for: an embedded
        // consumer renders its status line and nothing else.
        let mut driver = PanelDriver::new(RecordingPanel::new(info(400, 300)));
        let strip = vec![0u8; 400 * 20 * 4];
        let rect = PanelRect::new(0, 280, 400, 20);
        driver
            .present(
                Source::at(&strip, rect).unwrap(),
                Some(rect),
                UpdateClass::Fast,
            )
            .unwrap()
            .unwrap();
        assert_eq!(driver.panel().blits, vec![rect]);
    }

    // ---- Deadlines ----

    #[test]
    fn flush_within_retires_what_finished_and_leaves_the_rest() {
        /// A panel that never finishes anything.
        #[derive(Debug)]
        struct Wedged(u32);
        impl Panel for Wedged {
            fn info(&self) -> PanelInfo {
                info(8, 8)
            }
            fn blit(&mut self, _src: Source<'_>, _rect: PanelRect) -> Result<()> {
                Ok(())
            }
            fn submit(&mut self, _rect: PanelRect, _update: Update<'_>) -> Result<UpdateToken> {
                self.0 += 1;
                Ok(UpdateToken::new(self.0))
            }
            fn wait(&mut self, _token: UpdateToken, timeout: Option<Duration>) -> Result<Wait> {
                // Blocking forever is what the real thing does; with a
                // deadline it comes back and says so.
                assert!(timeout.is_some(), "the test would have hung");
                Ok(Wait::TimedOut)
            }
        }

        let buffer = vec![0u8; 8 * 8 * 4];
        let mut driver = PanelDriver::new(Wedged(0));
        driver
            .present(
                Source::panel(&buffer, 8, 8).unwrap(),
                None,
                UpdateClass::Fast,
            )
            .unwrap();
        assert_eq!(
            driver.flush_within(Duration::from_millis(1)).unwrap(),
            Wait::TimedOut
        );
        // Still live: a timed-out update is still reading the memory.
        assert_eq!(driver.live().len(), 1);
    }

    #[test]
    fn a_backend_that_ignores_an_unbounded_wait_stops_the_blit() {
        /// A backend breaking the contract: it reports a timeout on a wait
        /// that had no deadline. Carrying on would write under a live
        /// update, so the driver refuses rather than shrugging.
        #[derive(Debug)]
        struct Liar(u32);
        impl Panel for Liar {
            fn info(&self) -> PanelInfo {
                info(8, 8)
            }
            fn blit(&mut self, _src: Source<'_>, _rect: PanelRect) -> Result<()> {
                Ok(())
            }
            fn submit(&mut self, _rect: PanelRect, _update: Update<'_>) -> Result<UpdateToken> {
                self.0 += 1;
                Ok(UpdateToken::new(self.0))
            }
            fn wait(&mut self, _token: UpdateToken, _timeout: Option<Duration>) -> Result<Wait> {
                Ok(Wait::TimedOut)
            }
        }

        let buffer = vec![0u8; 8 * 8 * 4];
        let mut driver = PanelDriver::new(Liar(0));
        let full = Source::panel(&buffer, 8, 8).unwrap();
        driver.present(full, None, UpdateClass::Fast).unwrap();
        let err = driver.present(full, None, UpdateClass::Fast).unwrap_err();
        assert!(matches!(err, Error::WaitStalled { .. }), "{err:?}");
    }

    // ---- Reducing per update ----

    /// A panel whose depth belongs to the waveform rather than the
    /// session. It asks for full colour, keeps what `blit` staged, and
    /// reduces in `submit` — the first place the class is known.
    #[derive(Debug)]
    struct DeferredPanel {
        info: PanelInfo,
        staged: Vec<u8>,
        next_token: u32,
        /// One entry per update: the class, and how many distinct grey
        /// levels the panel actually put on the glass for it.
        reduced: Vec<(UpdateClass, usize)>,
    }

    impl DeferredPanel {
        fn new(width: u32, height: u32) -> Self {
            DeferredPanel {
                // "Do not reduce; I will."
                info: PanelInfo::new(width, height, PixelFormat::Rgba),
                staged: vec![0; (width * height * 4) as usize],
                next_token: 1,
                reduced: Vec::new(),
            }
        }

        /// What this device's waveform for `class` can actually resolve.
        fn levels_for(class: UpdateClass) -> u32 {
            match class {
                UpdateClass::Monochrome => 2,
                UpdateClass::Fast => 4,
                UpdateClass::Quality | UpdateClass::Flash => 16,
            }
        }
    }

    impl Panel for DeferredPanel {
        fn info(&self) -> PanelInfo {
            self.info
        }

        fn blit(&mut self, src: Source<'_>, rect: PanelRect) -> Result<()> {
            src.require(rect)?;
            // Nothing to reduce to yet, so keep the pixels as they came.
            let w = self.info.width as usize;
            for y in rect.y..rect.max_y() {
                let row = src.row(rect.x, y).expect("checked by require");
                let at = (y as usize * w + rect.x as usize) * 4;
                let len = rect.w as usize * 4;
                self.staged[at..at + len].copy_from_slice(&row[..len]);
            }
            Ok(())
        }

        fn submit(&mut self, rect: PanelRect, update: Update<'_>) -> Result<UpdateToken> {
            let levels = DeferredPanel::levels_for(update.class());
            let w = self.info.width as usize;
            let step = 255.0 / (levels - 1) as f32;
            let mut seen = Vec::new();
            for y in rect.y as usize..rect.max_y() as usize {
                for x in rect.x as usize..rect.max_x() as usize {
                    let lum = f32::from(self.staged[(y * w + x) * 4]);
                    let level = (lum / step).round().clamp(0.0, (levels - 1) as f32) * step;
                    seen.push(level as u8);
                }
            }
            seen.sort_unstable();
            seen.dedup();
            self.reduced.push((update.class(), seen.len()));
            let token = UpdateToken::new(self.next_token);
            self.next_token += 1;
            Ok(token)
        }

        fn wait(&mut self, _token: UpdateToken, _timeout: Option<Duration>) -> Result<Wait> {
            Ok(Wait::Complete)
        }
    }

    #[test]
    fn a_panel_may_defer_reduction_until_it_knows_the_class() {
        let (w, h) = (256u32, 4u32);
        let mut driver = PanelDriver::with_policy(DeferredPanel::new(w, h), RefreshPolicy::never());

        // A full ramp, so a reduction has something to lose.
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let at = ((y * w + x) * 4) as usize;
                let v = x as u8;
                rgba[at..at + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let source = Source::panel(&rgba, w, h).unwrap();

        let all = Some(PanelRect::full(w, h));
        driver
            .present(source, all, UpdateClass::Monochrome)
            .unwrap();
        driver.present(source, all, UpdateClass::Fast).unwrap();
        driver.present(source, all, UpdateClass::Quality).unwrap();

        // The same staged pixels resolved three different depths, each
        // matching its waveform. Reducing above the panel could only have
        // picked one, and would have been wrong for the other two.
        assert_eq!(
            driver.panel().reduced,
            vec![
                (UpdateClass::Monochrome, 2),
                (UpdateClass::Fast, 4),
                (UpdateClass::Quality, 16),
            ]
        );
        // And nothing above the panel quantized at all.
        assert_eq!(driver.info().format, PixelFormat::Rgba);
    }
}
