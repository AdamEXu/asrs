//! Comparing what has the focus in upstream's page with what has it in the port's, as the
//! `traverse` actions of scenarios keep it.

use crate::json::Json;

/// The port's elements sit on whole device pixels, to which their origins and their sizes or their
/// far edges are rounded.
const TOLERANCE: f64 = 0.501;

fn rect(focused: &Json) -> Option<[f64; 4]> {
    let rect = focused.get("rect")?.as_array()?;
    let rect: Option<Vec<f64>> = rect.iter().map(Json::as_f64).collect();
    rect?.try_into().ok()
}

/// Whether upstream's record is of an element that only a press focuses, whose focus `body` has in
/// the port.
fn nothing(upstream: &Json) -> bool {
    *upstream == Json::Null || upstream.get("stop") == Some(&Json::Bool(false))
}

/// What differs between upstream's record and the port's.
pub fn difference(upstream: &Json, port: &Json) -> Option<String> {
    if nothing(upstream) || *port == Json::Null {
        let same = nothing(upstream) && *port == Json::Null;
        return (!same).then(|| format!("{upstream:?} upstream, {port:?} on the port"));
    }
    let (Some(theirs), Some(ours)) = (rect(upstream), rect(port)) else {
        return Some(format!("no box in {upstream:?} or {port:?}"));
    };
    let close = (0..2).all(|axis| {
        let near = |t: f64, o: f64| (t - o).abs() <= TOLERANCE;
        let (t, o) = (theirs[axis], ours[axis]);
        let (t_size, o_size) = (theirs[axis + 2], ours[axis + 2]);
        near(t, o) && (near(t_size, o_size) || near(t + t_size, o + o_size))
    });
    if !close {
        return Some(format!("{upstream:?} upstream, {port:?} on the port"));
    }
    // Records from before `stop` was kept have no values either.
    let keys = match upstream.get("stop") {
        Some(_) => &["ring", "value", "selection"][..],
        None => &["ring"],
    };
    for key in keys {
        if upstream.get(key) != port.get(key) {
            return Some(format!(
                "{key}: {upstream:?} upstream, {port:?} on the port"
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Json {
        crate::json::parse(text).expect("JSON")
    }

    #[test]
    fn differences() {
        let theirs = json(r#"{"stop":true,"rect":[822.5,1,2,3],"ring":true}"#);
        for (ours, same) in [
            (r#"{"rect":[823,1,2,3],"ring":true}"#, true),
            (r#"{"rect":[824,1,2,3],"ring":true}"#, false),
            (r#"{"rect":[823,1,2,3],"ring":false}"#, false),
            (r#"{"rect":[823,1,2,3],"ring":true,"value":"a"}"#, false),
            ("null", false),
        ] {
            assert_eq!(difference(&theirs, &json(ours)).is_none(), same, "{ours}");
        }
        let field =
            json(r#"{"stop":true,"rect":[0,1,2,3],"ring":false,"value":"a","selection":[0,1]}"#);
        for (ours, same) in [(r#"[0,1]"#, true), (r#"[1,1]"#, false)] {
            let ours =
                format!(r#"{{"rect":[0,1,2,3],"ring":false,"value":"a","selection":{ours}}}"#);
            assert_eq!(difference(&field, &json(&ours)).is_none(), same, "{ours}");
        }
        let overlay = json(r#"{"stop":false,"rect":[0,1,2,3],"ring":false}"#);
        assert_eq!(difference(&overlay, &Json::Null), None);
        assert_eq!(difference(&Json::Null, &Json::Null), None);
        assert!(difference(&Json::Null, &json(r#"{"rect":[0,1,2,3],"ring":true}"#)).is_some());
    }
}
