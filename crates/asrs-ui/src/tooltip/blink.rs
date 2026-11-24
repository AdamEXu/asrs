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
        let mut update = |text: &str, requested: &mut bool| {
            if !text.is_empty() || *requested {
                *requested = !text.is_empty();
                update(text);
            }
        };
        if self.point == Some(point) && self.title.as_deref() == title {
            return;
        }
        let titled = self.title.as_deref().is_some_and(|title| !title.is_empty());
        if node != self.node && titled && self.title.as_deref() == title {
            update("", &mut self.requested);
        }
        self.point = Some(point);
        self.title = title.map(str::to_owned);
        self.node = node;
        update(title.unwrap_or_default(), &mut self.requested);
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
    fn a_pointer_at_rest_updates_only_a_changed_title() {
        let mut page = Page::default();
        assert_eq!(texts(&mut page, 1.0, Some(0.0), Some("Play")), ["Play"]);
        assert!(texts(&mut page, 1.0, Some(0.0), Some("Play")).is_empty());
        assert_eq!(texts(&mut page, 1.0, Some(0.0), Some("Pause")), ["Pause"]);
    }
}
