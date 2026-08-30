//! Ghosting debt: when to spend a flash the content did not ask for.

use crate::UpdateClass;

/// Every non-flashing update leaves a little charge behind, and after
/// enough of them the page acquires a grey shadow of what used to be
/// there. The cure is a full flash, and the cost is a visible black-white
/// blink, so it has to be rationed rather than avoided.
///
/// This is deliberately separate from whatever decides an update's class.
/// "What does this change need?" and "is the screen due for a clean?" are
/// different questions, and only the second one has a knob a user might
/// want to turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshPolicy {
    flash_every: u32,
    since_flash: u32,
}

impl RefreshPolicy {
    /// Six updates between flashes, matching what mainstream readers
    /// settle on: often enough that ghosting stays below notice, rare
    /// enough that the blink reads as punctuation.
    pub const DEFAULT_FLASH_EVERY: u32 = 6;

    /// Flash after `flash_every` updates that could have caused ghosting.
    /// Zero disables scheduled flashes entirely.
    pub const fn new(flash_every: u32) -> Self {
        RefreshPolicy {
            flash_every,
            since_flash: 0,
        }
    }

    /// Never flash unless the content itself calls for it.
    pub const fn never() -> Self {
        RefreshPolicy::new(0)
    }

    /// Decide what actually goes to the panel, escalating to a flash when
    /// the debt has come due.
    ///
    /// Only [`UpdateClass::Quality`] escalates. Fast and monochrome
    /// updates accrue debt but never trigger the flash themselves, because
    /// they are what a gesture in progress produces — flashing the screen
    /// in the middle of a selection drag would be worse than any amount of
    /// ghosting.
    pub fn resolve(&mut self, class: UpdateClass) -> UpdateClass {
        match class {
            UpdateClass::Flash => {
                self.since_flash = 0;
                UpdateClass::Flash
            }
            UpdateClass::Monochrome | UpdateClass::Fast => {
                self.since_flash = self.since_flash.saturating_add(1);
                class
            }
            UpdateClass::Quality => {
                self.since_flash = self.since_flash.saturating_add(1);
                if self.flash_every > 0 && self.since_flash >= self.flash_every {
                    self.since_flash = 0;
                    UpdateClass::Flash
                } else {
                    UpdateClass::Quality
                }
            }
        }
    }

    /// Updates since the last flash — for a shell that wants to flash on
    /// its own schedule, such as when the reader goes idle.
    pub const fn since_flash(&self) -> u32 {
        self.since_flash
    }

    /// The cadence this policy was built with.
    pub const fn flash_every(&self) -> u32 {
        self.flash_every
    }

    /// Forget the accumulated debt, as after a flash this policy did not
    /// decide on.
    pub fn reset(&mut self) {
        self.since_flash = 0;
    }

    /// Put back a scheduled flash that could not be spent, so the next
    /// eligible update comes due again.
    ///
    /// For a caller that asked [`resolve`](RefreshPolicy::resolve) what to
    /// do, was told to flash, and then could not — the commonest reason
    /// being that it holds pixels for part of the panel and a flash needs
    /// all of them. The debt is real either way; deferring it is right and
    /// forgetting it is not.
    pub fn defer_flash(&mut self) {
        if self.flash_every > 0 {
            self.since_flash = self.flash_every;
        }
    }
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        RefreshPolicy::new(RefreshPolicy::DEFAULT_FLASH_EVERY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_never_flashes() {
        // The whole reason fast classes do not escalate: flashing the
        // screen under a moving finger is worse than any ghosting.
        let mut policy = RefreshPolicy::new(6);
        for _ in 0..200 {
            assert_ne!(
                policy.resolve(UpdateClass::Monochrome),
                UpdateClass::Flash,
                "a selection drag triggered a flash"
            );
        }
        for _ in 0..200 {
            assert_ne!(policy.resolve(UpdateClass::Fast), UpdateClass::Flash);
        }
    }

    #[test]
    fn page_turns_flash_on_cadence() {
        let mut policy = RefreshPolicy::new(3);
        use UpdateClass::{Flash, Quality};
        let mut seen = [Quality; 7];
        for slot in &mut seen {
            *slot = policy.resolve(Quality);
        }
        assert_eq!(
            seen,
            [Quality, Quality, Flash, Quality, Quality, Flash, Quality]
        );
    }

    #[test]
    fn a_content_flash_pays_off_the_debt() {
        let mut policy = RefreshPolicy::new(3);
        policy.resolve(UpdateClass::Quality);
        policy.resolve(UpdateClass::Quality);
        // A relayout flashes on its own account, which cleans the panel.
        assert_eq!(policy.resolve(UpdateClass::Flash), UpdateClass::Flash);
        assert_eq!(policy.since_flash(), 0);
        assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Quality);
    }

    #[test]
    fn fast_updates_still_accrue_debt() {
        // They do not trigger the flash, but they do dirty the panel, so
        // the next settled update should come due sooner.
        let mut policy = RefreshPolicy::new(3);
        policy.resolve(UpdateClass::Monochrome);
        policy.resolve(UpdateClass::Fast);
        assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Flash);
    }

    #[test]
    fn zero_disables_scheduled_flashes() {
        let mut policy = RefreshPolicy::never();
        for _ in 0..500 {
            assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Quality);
        }
    }

    #[test]
    fn a_deferred_flash_comes_due_again_immediately() {
        // The debt does not evaporate because the moment was wrong.
        let mut policy = RefreshPolicy::new(3);
        for _ in 0..3 {
            policy.resolve(UpdateClass::Quality);
        }
        policy.defer_flash();
        assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Flash);
    }

    #[test]
    fn deferring_a_flash_does_nothing_when_none_are_scheduled() {
        let mut policy = RefreshPolicy::never();
        policy.defer_flash();
        assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Quality);
    }

    #[test]
    fn a_reset_forgets_a_flash_this_policy_did_not_decide() {
        // A shell that flashed on idle owns the clean it just bought.
        let mut policy = RefreshPolicy::new(3);
        policy.resolve(UpdateClass::Quality);
        policy.resolve(UpdateClass::Quality);
        policy.reset();
        assert_eq!(policy.resolve(UpdateClass::Quality), UpdateClass::Quality);
    }
}
