use std::fmt::Write;

/// Upstream's `build.fileAssociations` without `.hoot`, which asrs cannot open. asrs registers as
/// an alternate handler of each, never as the default.
pub struct Association {
    pub extension: &'static str,
    pub name: &'static str,
    pub mime: &'static str,
}

const fn association(
    extension: &'static str,
    name: &'static str,
    mime: &'static str,
) -> Association {
    Association {
        extension,
        name,
        mime,
    }
}

pub const ASSOCIATIONS: &[Association] = &[
    association("wpilog", "WPILib robot log", "application/x-wpilog"),
    association(
        "wpilogxz",
        "Compressed WPILib robot log",
        "application/x-wpilogxz",
    ),
    association("rlog", "Robot log", "application/x-rlog"),
    association("revlog", "REV Robotics CAN log", "application/x-revlog"),
    association("csv", "Comma-separated values", CSV),
    association("dslog", "NI Driver Station log", "application/x-dslog"),
    association(
        "dsevents",
        "NI Driver Station events",
        "application/x-dsevents",
    ),
];

/// The one type that is not a robot log's: every desktop already defines it, and other programs
/// open it.
pub const CSV: &str = "text/csv";

pub fn extensions() -> impl Iterator<Item = &'static str> {
    ASSOCIATIONS.iter().map(|a| a.extension)
}

/// The types of the Linux desktop entry. An entry cannot be an alternate: without another program
/// for exactly this type, the one that names it is its default. So CSV files are left out.
pub fn desktop_types(separator: &str) -> String {
    let types: Vec<&str> = ASSOCIATIONS
        .iter()
        .map(|a| a.mime)
        .filter(|mime| *mime != CSV)
        .collect();
    types.join(separator)
}

/// The shared-mime-info package of the types asrs adds.
pub fn mime_package() -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n",
    );
    for a in ASSOCIATIONS {
        if a.mime == CSV {
            continue;
        }
        let _ = write!(
            out,
            "  <mime-type type=\"{}\">\n    <comment>{}</comment>\n    <glob pattern=\"*.{}\"/>\n  </mime-type>\n",
            a.mime, a.name, a.extension
        );
    }
    out.push_str("</mime-info>\n");
    out
}

#[cfg(test)]
mod tests {
    use asrs_parity::json::{self, Json};

    use super::*;
    use crate::workspace_root;

    #[test]
    fn the_associations_are_upstreams_without_hoot() {
        let path = workspace_root().join("reference/AdvantageScope/package.json");
        let text = std::fs::read_to_string(&path).expect("upstream's package.json");
        let package = json::parse(&text).expect("JSON");
        let field = |entry: &Json, key: &str| {
            let value = entry.get(key).and_then(Json::as_str).expect("string");
            value.to_string_lossy()
        };
        let upstream: Vec<[String; 3]> = package
            .get("build")
            .and_then(|build| build.get("fileAssociations"))
            .and_then(Json::as_array)
            .expect("fileAssociations")
            .iter()
            .map(|entry| ["ext", "name", "mimeType"].map(|key| field(entry, key)))
            .collect();
        assert!(upstream.iter().any(|[ext, ..]| ext == "hoot"));
        let ours: Vec<[String; 3]> = ASSOCIATIONS
            .iter()
            .map(|a| [a.extension, a.name, a.mime].map(str::to_owned))
            .collect();
        let expected: Vec<[String; 3]> = upstream
            .into_iter()
            .filter(|[ext, ..]| ext != "hoot")
            .collect();
        assert_eq!(ours, expected);
    }
}
