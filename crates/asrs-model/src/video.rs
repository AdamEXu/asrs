//! `src/shared/VideoSource.ts` and the pure match-key half of `src/shared/TBAUtil.ts`
//! (`getTBAMatchKey` once the TBA event is known; the fetches live in the video
//! processor).

use asrs_js::number;
use asrs_js::string::JsString;
use asrs_log::match_info::{MatchInfo, MatchType, get_elim_match_string};

/// The `source` member of a `select-video` message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoSource {
    Local,
    YouTube,
    TheBlueAlliance,
}

impl VideoSource {
    pub const ALL: &'static [VideoSource] = &[
        VideoSource::Local,
        VideoSource::YouTube,
        VideoSource::TheBlueAlliance,
    ];

    pub fn number(self) -> f64 {
        match self {
            VideoSource::Local => 0.0,
            VideoSource::YouTube => 1.0,
            VideoSource::TheBlueAlliance => 2.0,
        }
    }

    pub fn from_number(value: f64) -> Option<VideoSource> {
        VideoSource::ALL
            .iter()
            .copied()
            .find(|s| s.number() == value)
    }
}

/// The match key `getTBAMatchKey` builds for a known TBA event:
/// `<eventKey>_qm<matchNumber>` for non-elimination matches,
/// `<eventKey>_<getElimMatchString(playoffType, matchNumber)>` for eliminations.
/// `playoff_type` is the event record's `playoff_type`; a missing or non-numeric one
/// reads as NaN, which the elim tables treat like any unknown value. `None` where
/// upstream's `getElimMatchString` throws on a missing table entry.
pub fn tba_match_key(
    event_key: &JsString,
    playoff_type: f64,
    match_info: &MatchInfo,
) -> Option<JsString> {
    let mut key = event_key.clone();
    key.push_str(&JsString::from("_"));
    match match_info.match_type {
        MatchType::Elimination => {
            key.push_str(&get_elim_match_string(playoff_type, match_info.match_number)?)
        }
        _ => {
            key.push_str(&JsString::from("qm"));
            key.push_str(&JsString::from(number::to_string(match_info.match_number)));
        }
    }
    Some(key)
}
