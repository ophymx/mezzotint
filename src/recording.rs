//! A [`Panel`] that keeps a log instead of a screen.

use alloc::vec::Vec;
use core::time::Duration;

use crate::{Panel, PanelInfo, PanelRect, Result, Source, Update, UpdateClass, UpdateToken, Wait};

/// One submission, as the panel saw it.
///
/// `#[non_exhaustive]`: this is a test rig, so it grows whenever the
/// contract does, and a test suite outside this crate should keep
/// compiling when it does. Reading the fields is the whole use; nothing
/// outside builds one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Recorded {
    /// The region the driver asked for — the requested one, which is what
    /// the rule against writing under a live update is enforced against.
    pub rect: PanelRect,
    /// The class after [`RefreshPolicy`](crate::RefreshPolicy) had its
    /// say, so an escalation to a flash shows up here.
    pub class: UpdateClass,
    /// Where the consumer said error diffusion was allowed to run.
    pub dither: Vec<PanelRect>,
    /// The token handed back for it.
    pub token: UpdateToken,
}

/// Refresh behaviour is the part of a device port that is both easy to get
/// wrong and impossible to eyeball — nobody can see, by looking at an
/// ereader, that a drag issued one update per pixel of travel. Driving a
/// shell against this makes those questions ordinary assertions that run
/// on a build machine with no panel attached.
///
/// It is also the conformance suite for the contract itself: it checks
/// what it is handed rather than trusting it, so a consumer that hands
/// over a buffer which cannot supply the region it named finds out here
/// instead of on glass.
#[derive(Debug, Clone)]
pub struct RecordingPanel {
    info: PanelInfo,
    next_token: u32,
    /// Every update submitted, oldest first.
    pub updates: Vec<Recorded>,
    /// Every region written, oldest first.
    pub blits: Vec<PanelRect>,
    /// Tokens passed to [`Panel::wait`], oldest first, with the deadline
    /// each was given.
    pub waits: Vec<(UpdateToken, Option<Duration>)>,
}

impl RecordingPanel {
    /// A panel of this shape that records rather than shows.
    pub fn new(info: PanelInfo) -> Self {
        RecordingPanel {
            info,
            next_token: 1,
            updates: Vec::new(),
            blits: Vec::new(),
            waits: Vec::new(),
        }
    }

    /// How many updates of a given class have been submitted.
    pub fn count(&self, class: UpdateClass) -> usize {
        self.updates.iter().filter(|u| u.class == class).count()
    }

    /// The classes submitted, oldest first.
    pub fn classes(&self) -> Vec<UpdateClass> {
        self.updates.iter().map(|u| u.class).collect()
    }

    /// The regions submitted, oldest first.
    pub fn rects(&self) -> Vec<PanelRect> {
        self.updates.iter().map(|u| u.rect).collect()
    }

    /// The tokens waited on, oldest first, without their deadlines.
    pub fn waited(&self) -> Vec<UpdateToken> {
        self.waits.iter().map(|(token, _)| *token).collect()
    }

    /// Forget everything recorded so far, keeping the token counter, so
    /// a test can assert about one phase without the previous one's log.
    pub fn clear(&mut self) {
        self.updates.clear();
        self.blits.clear();
        self.waits.clear();
    }
}

impl Panel for RecordingPanel {
    fn info(&self) -> PanelInfo {
        self.info
    }

    fn blit(&mut self, src: Source<'_>, rect: PanelRect) -> Result<()> {
        src.require(rect)?;
        self.blits.push(rect);
        Ok(())
    }

    fn submit(&mut self, rect: PanelRect, update: Update<'_>) -> Result<UpdateToken> {
        let token = UpdateToken::new(self.next_token);
        self.next_token += 1;
        self.updates.push(Recorded {
            rect,
            class: update.class(),
            dither: update.dither_regions().to_vec(),
            token,
        });
        Ok(token)
    }

    fn wait(&mut self, token: UpdateToken, timeout: Option<Duration>) -> Result<Wait> {
        self.waits.push((token, timeout));
        Ok(Wait::Complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Error, PixelFormat};
    use alloc::vec;

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

    #[test]
    fn a_recording_panel_logs_what_a_shell_did() {
        let mut panel = RecordingPanel::new(info(4, 4));
        let buffer = vec![0u8; 4 * 4 * 4];
        let src = Source::panel(&buffer, 4, 4).unwrap();
        let rect = PanelRect::new(0, 0, 4, 2);
        panel.blit(src, rect).unwrap();
        let token = panel.submit(rect, Update::new(UpdateClass::Fast)).unwrap();
        assert!(panel.wait(token, None).unwrap().is_complete());

        assert_eq!(panel.blits, vec![rect]);
        assert_eq!(panel.rects(), vec![rect]);
        assert_eq!(panel.classes(), vec![UpdateClass::Fast]);
        assert_eq!(panel.waited(), vec![token]);
        assert_eq!(panel.count(UpdateClass::Fast), 1);
        assert_eq!(panel.count(UpdateClass::Flash), 0);
    }

    #[test]
    fn tokens_are_distinct() {
        let mut panel = RecordingPanel::new(info(4, 4));
        let rect = PanelRect::full(4, 4);
        let first = panel.submit(rect, UpdateClass::Quality.into()).unwrap();
        let second = panel.submit(rect, UpdateClass::Quality.into()).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn the_diffusion_hints_reach_the_panel() {
        // The channel by which a consumer says where a photograph is.
        // If it does not survive the trip there is no point having it.
        let mut panel = RecordingPanel::new(info(64, 64));
        let images = [PanelRect::new(8, 8, 16, 16)];
        panel
            .submit(
                PanelRect::full(64, 64),
                Update::new(UpdateClass::Quality).dithering_within(&images),
            )
            .unwrap();
        assert_eq!(panel.updates[0].dither, images);
    }

    #[test]
    fn a_source_that_cannot_supply_the_region_is_rejected() {
        // Catches the shell that hands over a page-sized buffer when the
        // panel wanted a rotated one — silent garbage on real hardware.
        let mut panel = RecordingPanel::new(info(4, 8));
        let buffer = vec![0u8; 4 * 4 * 4];
        let src = Source::panel(&buffer, 4, 4).unwrap();
        let err = panel.blit(src, PanelRect::full(4, 8)).unwrap_err();
        assert!(matches!(err, Error::NotCovered { .. }), "{err:?}");
        assert!(panel.blits.is_empty(), "a rejected blit was logged");
    }
}
