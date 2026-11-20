//! `shared/MatchInfo.ts`: the `MatchInfo` record and its enums.

use asrs_js::string::JsString;

/// `MatchInfo`: the match metadata `RobotState.getMatchInfo` extracts.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchInfo {
    pub year: f64,
    pub event: JsString,
    pub match_type: MatchType,
    pub match_number: f64,
}

/// `MatchType`; discriminants are the serialized values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MatchType {
    Practice = 1,
    Qualification = 2,
    Elimination = 3,
}

impl MatchType {
    /// The enum value for a logged number, or `None` outside 1..=3
    /// (`[1, 2, 3].includes(matchType)` upstream).
    pub fn from_number(value: f64) -> Option<Self> {
        if value == 1.0 {
            Some(MatchType::Practice)
        } else if value == 2.0 {
            Some(MatchType::Qualification)
        } else if value == 3.0 {
            Some(MatchType::Elimination)
        } else {
            None
        }
    }

    pub fn as_number(self) -> f64 {
        self as u8 as f64
    }
}

/// `PlayoffType`, TBA's playoff bracket identifiers. Discriminants are the
/// serialized values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PlayoffType {
    Bracket8Team = 0,
    Bracket16Team = 1,
    Bracket4Team = 2,
    AvgScore8Team = 3,
    RoundRobin6Team = 4,
    LegacyDoubleElim8Team = 5,
    Bo5Finals = 6,
    Bo3Finals = 7,
    Custom = 8,
    Bracket2Team = 9,
    DoubleElim8Team = 10,
    DoubleElim4Team = 11,
}

impl PlayoffType {
    pub fn as_number(self) -> f64 {
        self as u8 as f64
    }
}
