use gpui_kit::{Bounds, Pixels, Point};

/// `ChromeClient::UpdateTooltipUnderCursor`: what a page tells the browser as the pointer moves
/// over it.
#[derive(Default)]
pub(super) struct Page {
    point: Option<Point<Pixels>>,
    title: Option<String>,
    node: Option<Bounds<Pixels>>,
    /// `ChromeClientImpl`'s `did_request_non_empty_tool_tip_`.
    requested: bool,
}

impl Page {
    /// The pointer is at `point`, on the element at `node`, under the `title` of that element or
    /// its nearest ancestor with one. Calls `update` with each text the browser gets.
    pub(super) fn moved(
        &mut self,
        point: Point<Pixels>,
        node: Option<Bounds<Pixels>>,
        title: Option<&str>,
        mut update: impl FnMut(&str),
    ) {
        if self.point == Some(point) && self.title.as_deref() == title {
            return;
        }
        let titled = self.title.as_deref().is_some_and(|title| !title.is_empty());
        if node != self.node && titled && self.title.as_deref() == title {
            self.give("", &mut update);
        }
        self.point = Some(point);
        self.title = title.map(str::to_owned);
        self.node = node;
        self.give(title.unwrap_or_default(), &mut update);
    }

    /// The pointer left the page (`ClearToolTip`).
    pub(super) fn left(&mut self, mut update: impl FnMut(&str)) {
        self.give("", &mut update);
    }

    fn give(&mut self, text: &str, update: &mut impl FnMut(&str)) {
        if !text.is_empty() || self.requested {
            self.requested = !text.is_empty();
            update(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{point, px, size};

    fn texts(page: &mut Page, x: f32, node: Option<f32>, title: Option<&str>) -> Vec<String> {
        let node = node.map(|x| Bounds::new(point(px(x), px(0.0)), size(px(9.0), px(9.0))));
        let mut out = Vec::new();
        page.moved(point(px(x), px(0.0)), node, title, |text| {
            out.push(text.to_owned());
        });
        out
    }

    #[test]
    fn the_same_title_on_another_node_is_cleared_first() {
        let mut page = Page::default();
        assert_eq!(texts(&mut page, 1.0, Some(0.0), Some("a")), ["a"]);
        assert_eq!(texts(&mut page, 2.0, Some(0.0), Some("a")), ["a"]);
        assert_eq!(texts(&mut page, 3.0, Some(3.0), Some("a")), ["", "a"]);
        assert_eq!(texts(&mut page, 4.0, Some(4.0), Some("b")), ["b"]);
        assert_eq!(texts(&mut page, 5.0, None, None), [""]);
        assert!(texts(&mut page, 6.0, None, None).is_empty());
    }

    #[test]
    fn leaving_clears_a_title_once() {
        let mut page = Page::default();
        let left = |page: &mut Page| {
            let mut out = Vec::new();
            page.left(|text| out.push(text.to_owned()));
            out
        };
        assert!(left(&mut page).is_empty());
        assert_eq!(texts(&mut page, 1.0, Some(0.0), Some("a")), ["a"]);
        assert_eq!(left(&mut page), [""]);
        assert!(left(&mut page).is_empty());
        assert_eq!(texts(&mut page, 2.0, Some(0.0), Some("a")), ["a"]);
    }

    #[test]
    fn a_pointer_at_rest_updates_only_a_changed_title() {
        let mut page = Page::default();
        assert_eq!(texts(&mut page, 1.0, Some(0.0), Some("Play")), ["Play"]);
        assert!(texts(&mut page, 1.0, Some(0.0), Some("Play")).is_empty());
        assert_eq!(texts(&mut page, 1.0, Some(0.0), Some("Pause")), ["Pause"]);
    }
}
