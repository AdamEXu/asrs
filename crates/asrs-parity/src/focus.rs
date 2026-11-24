//! Comparing what has the focus in upstream's page with what has it in the port's, as the
//! `traverse` actions of scenarios keep it.

use crate::json::Json;

/// The port's elements sit on whole device pixels.
const TOLERANCE: f64 = 0.501;

fn rect(focused: &Json) -> Option<Vec<f64>> {
    let rect = focused.get("rect")?.as_array()?;
    rect.iter().map(Json::as_f64).collect()
}

/// Whether upstream's record is of an element that takes no focus from Tab, which is `body`'s in
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
    let close = theirs.len() == ours.len()
        && theirs
            .iter()
            .zip(&ours)
            .all(|(t, o)| (t - o).abs() <= TOLERANCE);
    if !close {
        return Some(format!("{upstream:?} upstream, at {ours:?} on the port"));
    }
    (upstream.get("ring") != port.get("ring")).then(|| format!("the ring of {upstream:?}"))
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
            ("null", false),
        ] {
            assert_eq!(difference(&theirs, &json(ours)).is_none(), same, "{ours}");
        }
        let scroller = json(r#"{"stop":false,"rect":[0,1,2,3],"ring":false}"#);
        assert_eq!(difference(&scroller, &Json::Null), None);
        assert_eq!(difference(&Json::Null, &Json::Null), None);
        assert!(difference(&Json::Null, &json(r#"{"rect":[0,1,2,3],"ring":true}"#)).is_some());
    }
}
