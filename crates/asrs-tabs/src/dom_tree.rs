//! The DOM subtree a DOM-drawing renderer owns: elements with ordered attributes, an inline
//! style declaration list and children, kept the way Blink keeps them so the attribute strings
//! a renderer leaves behind equal upstream's.

use asrs_js::string::{JsError, JsString};

/// The text an `element.innerText = value` assignment shows: every CR, LF or CRLF becomes a
/// `<br>`, written here as `\n`.
pub fn inner_text(value: &JsString) -> JsString {
    let units = value.units();
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        if units[i] == 0x0d {
            out.push(0x0a);
            i += usize::from(units.get(i + 1) == Some(&0x0a));
        } else {
            out.push(units[i]);
        }
        i += 1;
    }
    JsString::from_units(out)
}

/// An element's inline style (`element.style`): declarations in insertion order, each holding
/// Blink's serialized value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineStyle {
    declarations: Vec<(String, String)>,
}

impl InlineStyle {
    /// `style.setProperty(property, value)`, which is what `style.<camelCase> = value` does:
    /// an empty value removes the declaration, a value Blink rejects changes nothing, and an
    /// accepted one replaces the declaration in place or appends it. `quirks` and errors as
    /// in [`crate::css_value::serialize`]. Returns whether the declaration list was mutated.
    pub fn set(&mut self, property: &str, value: &str, quirks: bool) -> Result<bool, JsError> {
        let existing = self.declarations.iter().position(|(p, _)| p == property);
        if value.is_empty() {
            return Ok(existing.map(|i| self.declarations.remove(i)).is_some());
        }
        let Some(serialized) = crate::css_value::serialize(property, value, quirks)? else {
            return Ok(false);
        };
        match existing {
            Some(i) => self.declarations[i].1 = serialized,
            None => self.declarations.push((property.to_owned(), serialized)),
        }
        Ok(true)
    }

    /// `style.getPropertyValue(property)`.
    pub fn get(&self, property: &str) -> &str {
        self.declarations
            .iter()
            .find(|(p, _)| p == property)
            .map_or("", |(_, v)| v)
    }

    /// `style.cssText`.
    pub fn css_text(&self) -> String {
        self.declarations
            .iter()
            .map(|(p, v)| format!("{p}: {v};"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn declarations(&self) -> &[(String, String)] {
        &self.declarations
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    Html,
    Svg,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    namespace: Namespace,
    tag: String,
    attributes: Vec<(String, String)>,
    style: InlineStyle,
    style_dirty: bool,
    children: Vec<Element>,
}

impl Element {
    pub fn new(namespace: Namespace, tag: &str) -> Element {
        Element::with_attributes(namespace, tag, &[])
    }

    /// An element as parsed from markup, attributes in source order.
    pub fn with_attributes(
        namespace: Namespace,
        tag: &str,
        attributes: &[(&str, &str)],
    ) -> Element {
        Element {
            namespace,
            tag: tag.to_owned(),
            attributes: attributes
                .iter()
                .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
                .collect(),
            style: InlineStyle::default(),
            style_dirty: false,
            children: Vec::new(),
        }
    }

    pub fn namespace(&self) -> Namespace {
        self.namespace
    }

    /// `localName`.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// `setAttribute(name, value)`: replaces in place or appends.
    pub fn set_attribute(&mut self, name: &str, value: &str) {
        match self.attributes.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value.to_owned(),
            None => self.attributes.push((name.to_owned(), value.to_owned())),
        }
    }

    pub fn style(&self) -> &InlineStyle {
        &self.style
    }

    /// `element.style.setProperty(property, value)`, parsed with quirks on SVG elements. The
    /// `style` attribute is not touched until the next [`Element::sync_style_attribute`].
    pub fn set_style(&mut self, property: &str, value: &str) -> Result<(), JsError> {
        let quirks = self.namespace == Namespace::Svg;
        if self.style.set(property, value, quirks)? {
            self.style_dirty = true;
        }
        Ok(())
    }

    /// What Blink does before any attribute read: once the inline style was mutated, the
    /// `style` attribute takes its `cssText`, in place if it exists and appended otherwise
    /// (so an emptied declaration list still leaves `style=""`).
    pub fn sync_style_attribute(&mut self) {
        if std::mem::take(&mut self.style_dirty) {
            let text = self.style.css_text();
            self.set_attribute("style", &text);
        }
    }

    /// `getAttributeNames()` with each `getAttribute()`, which first syncs the `style`
    /// attribute.
    pub fn attributes(&mut self) -> &[(String, String)] {
        self.sync_style_attribute();
        &self.attributes
    }

    pub fn children(&self) -> &[Element] {
        &self.children
    }

    pub fn children_mut(&mut self) -> &mut Vec<Element> {
        &mut self.children
    }

    pub fn append_child(&mut self, child: Element) -> &mut Element {
        self.children.push(child);
        self.children.last_mut().expect("just appended")
    }

    /// `while (el.firstChild) el.removeChild(el.firstChild)`.
    pub fn clear_children(&mut self) {
        self.children.clear();
    }
}
