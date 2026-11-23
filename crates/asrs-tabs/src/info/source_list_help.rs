use asrs_js::string::{JsError, JsString};
use asrs_log::colors::ensure_theme_contrast;
use asrs_log::log::LoggableType;
use asrs_model::source_list_config::{SourceListConfig, SourceListTypeConfig};

/// `Object.values(LoggableType)`.
const LOGGABLE_TYPES: [LoggableType; 8] = [
    LoggableType::Raw,
    LoggableType::Boolean,
    LoggableType::Number,
    LoggableType::String,
    LoggableType::BooleanArray,
    LoggableType::NumberArray,
    LoggableType::StringArray,
    LoggableType::Empty,
];

/// A colour per theme, light then dark.
pub type ThemeColors = [String; 2];

/// What `addItem` builds for one type.
#[derive(Clone, Debug, PartialEq)]
pub struct HelpItem {
    pub title: String,
    /// `symbols/sourceList/<symbol>.svg`.
    pub symbol: String,
    /// The icon's `color`.
    pub colors: ThemeColors,
    /// `div.parent-warning`'s text.
    pub parent_warning: Option<String>,
    pub options: Vec<HelpOption>,
    /// `div.source-types`'s text.
    pub sources: String,
}

/// A row of `table.options`.
#[derive(Clone, Debug, PartialEq)]
pub struct HelpOption {
    pub name: String,
    /// The value cell's text nodes.
    pub values: Vec<HelpText>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HelpText {
    pub text: String,
    /// Whether it is in a `<span>`, as values are and the text between them is not.
    pub span: bool,
    /// The span's `color`, if its value's key is one.
    pub colors: Option<ThemeColors>,
}

/// The page. A `set-config` message adds its types below those already there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceListHelp {
    /// `config.title`, once the page has a config.
    title: Option<String>,
    items: Vec<HelpItem>,
}

impl SourceListHelp {
    /// `document.title` up to the dash before the app name.
    pub fn title(&self) -> String {
        match &self.title {
            Some(title) => format!("{title} Help"),
            None => "Help".to_owned(),
        }
    }

    pub fn items(&self) -> &[HelpItem] {
        &self.items
    }

    /// `set-config`. An error is the handler's throw, after the types before it were added.
    pub fn set_config(&mut self, config: &SourceListConfig) -> Result<(), JsError> {
        self.title = Some(config.title.clone());
        let mut source_types: Vec<Vec<String>> = config
            .types
            .iter()
            .map(|t| t.source_types.clone())
            .collect();
        let mut used_colors = Vec::new();
        for (index, type_config) in config.types.iter().enumerate() {
            if !type_config.show_docs {
                continue;
            }
            let light = light_color(type_config, &mut used_colors)?;
            let dark = type_config.dark_color.as_deref().unwrap_or(&light);
            let colors = [
                ensure_theme_contrast(&light, false),
                ensure_theme_contrast(dark, true),
            ];

            // Upstream pushes onto the config's own list, so types after this one that share
            // its prefix get what it collected too.
            for (extra, extra_config) in config.types.iter().enumerate() {
                if extra != index && extra_config.key.starts_with(&type_config.key) {
                    for t in source_types[extra].clone() {
                        if !source_types[index].contains(&t) {
                            source_types[index].push(t);
                        }
                    }
                }
            }
            let sources: Vec<String> = source_types[index].iter().map(|t| source_name(t)).collect();

            let mut parents: Vec<String> = Vec::new();
            if let Some(child_of) = &type_config.child_of {
                for parent in &config.types {
                    let quoted = format!("\"{}\"", parent.display);
                    if parent.parent_key.as_ref() == Some(child_of) && !parents.contains(&quoted) {
                        parents.push(quoted);
                    }
                }
            }
            self.items.push(HelpItem {
                title: type_config.display.clone(),
                symbol: type_config.symbol.clone(),
                colors,
                parent_warning: (!parents.is_empty())
                    .then(|| format!("Add to existing {} item.", comma_list(&parents))),
                options: options(type_config),
                sources: format!(
                    "Source{}: {}",
                    if sources.len() == 1 { "" } else { "s" },
                    comma_list(&sources)
                ),
            });
        }
        Ok(())
    }
}

/// The type's own colour, or the first value of its colour option no earlier type has taken.
fn light_color(
    type_config: &SourceListTypeConfig,
    used_colors: &mut Vec<String>,
) -> Result<String, JsError> {
    if type_config.color.starts_with('#') {
        return Ok(type_config.color.clone());
    }
    let option = type_config
        .options
        .iter()
        .find(|option| option.key == type_config.color);
    let Some(option) = option else {
        return Ok("#000000".to_owned());
    };
    let color = option
        .values
        .iter()
        .map(|value| &value.key)
        .find(|key| !used_colors.contains(key))
        .ok_or_else(|| {
            JsError::type_error("Cannot read properties of undefined (reading 'key')")
        })?;
    used_colors.push(color.clone());
    Ok(color.clone())
}

fn source_name(source_type: &str) -> String {
    let loggable = LOGGABLE_TYPES.iter().any(|t| t.name() == source_type);
    if loggable {
        let text = JsString::from(source_type.replace("Array", "[]").as_str());
        text.to_lower_case().to_string_lossy()
    } else {
        source_type.split(':').next().unwrap_or("").to_owned()
    }
}

fn options(type_config: &SourceListTypeConfig) -> Vec<HelpOption> {
    type_config
        .options
        .iter()
        .map(|option| {
            let count = option.values.len();
            let mut values = Vec::new();
            for (i, value) in option.values.iter().enumerate() {
                if let Some(separator) = separator(i, count) {
                    values.push(HelpText {
                        text: separator.to_owned(),
                        span: false,
                        colors: None,
                    });
                }
                values.push(HelpText {
                    text: html_text(&value.display),
                    span: true,
                    colors: value
                        .key
                        .starts_with('#')
                        .then(|| [false, true].map(|dark| ensure_theme_contrast(&value.key, dark))),
                });
            }
            HelpOption {
                name: html_text(&format!("{}:", option.display)),
                values,
            }
        })
        .collect()
}

/// What `makeCommaList` puts before the value at `index` of `count`.
fn separator(index: usize, count: usize) -> Option<&'static str> {
    match index {
        0 => None,
        1 if count == 2 => Some(" or "),
        i if i + 1 == count => Some(", or "),
        _ => Some(", "),
    }
}

fn comma_list(values: &[String]) -> String {
    let mut out = String::new();
    for (i, value) in values.iter().enumerate() {
        out.push_str(separator(i, values.len()).unwrap_or(""));
        out.push_str(value);
    }
    out
}

/// The text of `innerHTML = html` for text without tags: option names and values come from the
/// configs and from asset names.
fn html_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match character_reference(rest) {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

const NAMED: [(&str, char); 7] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&apos;", '\''),
    ("&nbsp;", '\u{a0}'),
    ("&copy;", '\u{a9}'),
];

fn character_reference(text: &str) -> Option<(char, usize)> {
    if let Some((name, c)) = NAMED.iter().find(|(name, _)| text.starts_with(name)) {
        return Some((*c, name.len()));
    }
    let end = text.find(';')?;
    let digits = text[..end].strip_prefix("&#")?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    Some((char::from_u32(code)?, end + 1))
}
