//! When a page whose frame loop runs every frame draws. Chromium repaints only what a frame
//! changed; a page's frames here leave its window alone while nothing they show has changed since
//! it was last drawn.

/// Frames that follow the last change. A frame reads the layout the one before left, and what
/// drawing does, such as applying a scroll, shows in the next.
const SETTLE_FRAMES: u8 = 4;

#[derive(Debug, Default)]
pub(crate) struct Repaint {
    settling: u8,
    /// The next draw was asked for by [`Repaint::frame`] only to settle.
    asked: bool,
    drawing: bool,
    draws: u64,
}

impl Repaint {
    pub(crate) fn settling(&self) -> bool {
        self.settling > 0
    }

    /// How often the page has been drawn.
    pub(crate) fn draws(&self) -> u64 {
        self.draws
    }

    /// A task ran. The ones drawing runs are part of the draw.
    pub(crate) fn task(&mut self) {
        if !self.drawing {
            self.changed();
        }
    }

    pub(crate) fn changed(&mut self) {
        self.settling = SETTLE_FRAMES;
        self.asked = false;
    }

    pub(crate) fn begin_draw(&mut self) {
        if !std::mem::take(&mut self.asked) {
            self.changed();
        }
        self.drawing = true;
        self.draws += 1;
    }

    pub(crate) fn end_draw(&mut self) {
        self.drawing = false;
    }

    /// Whether the page is to be drawn after a frame. `changed` says whether something did since
    /// the last one.
    pub(crate) fn frame(&mut self, changed: bool) -> bool {
        if changed {
            self.changed();
            return true;
        }
        if self.settling == 0 {
            return false;
        }
        self.settling -= 1;
        self.asked = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frames drawn out of the next `count`, in which nothing changes.
    fn idle(repaint: &mut Repaint, count: usize) -> usize {
        let frames = (0..count).filter(|_| {
            let drawn = repaint.frame(false);
            if drawn {
                repaint.begin_draw();
                repaint.task();
                repaint.end_draw();
            }
            drawn
        });
        frames.count()
    }

    #[test]
    fn a_change_is_followed_by_a_few_frames() {
        let mut repaint = Repaint::default();
        assert_eq!(idle(&mut repaint, 100), 0);
        repaint.task();
        assert!(repaint.settling());
        assert_eq!(idle(&mut repaint, 100), usize::from(SETTLE_FRAMES));
        assert!(!repaint.settling());
        assert!(repaint.frame(true));
        assert_eq!(idle(&mut repaint, 100), usize::from(SETTLE_FRAMES));
        repaint.begin_draw();
        repaint.end_draw();
        assert_eq!(idle(&mut repaint, 100), usize::from(SETTLE_FRAMES));
    }

    #[test]
    fn a_change_during_the_last_frame_is_settled_too() {
        let mut repaint = Repaint::default();
        repaint.task();
        for _ in 0..SETTLE_FRAMES {
            assert!(repaint.frame(false));
        }
        repaint.task();
        repaint.begin_draw();
        repaint.end_draw();
        assert_eq!(idle(&mut repaint, 100), usize::from(SETTLE_FRAMES));
    }
}
