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

#[derive(Clone, Copy, PartialEq, Eq)]
enum CompLevel {
    Ef,
    Qf,
    Sf,
    F,
}

impl CompLevel {
    fn as_str(self) -> &'static str {
        match self {
            CompLevel::Ef => "ef",
            CompLevel::Qf => "qf",
            CompLevel::Sf => "sf",
            CompLevel::F => "f",
        }
    }
}

/// `getElimMatchString`: strings like `sf5m2` for an elimination match, or
/// `None` where upstream throws (a match number missing from the bracket's
/// mapping).
pub fn get_elim_match_string(playoff_type: f64, match_number: f64) -> Option<JsString> {
    let comp_level = get_elim_comp_level(playoff_type, match_number)?;
    let (set, m) = get_elim_match_numbers(playoff_type, comp_level, match_number)?;
    Some(JsString::from(format!(
        "{}{}m{}",
        comp_level.as_str(),
        asrs_js::number::to_string(set),
        asrs_js::number::to_string(m)
    )))
}

fn get_elim_comp_level(playoff_type: f64, match_number: f64) -> Option<CompLevel> {
    if playoff_type == PlayoffType::AvgScore8Team.as_number() {
        if match_number <= 8.0 {
            Some(CompLevel::Qf)
        } else if match_number <= 14.0 {
            Some(CompLevel::Sf)
        } else {
            Some(CompLevel::F)
        }
    } else if playoff_type == PlayoffType::RoundRobin6Team.as_number() {
        Some(if match_number <= 15.0 {
            CompLevel::Sf
        } else {
            CompLevel::F
        })
    } else if playoff_type == PlayoffType::DoubleElim8Team.as_number() {
        Some(DOUBLE_ELIM_MAPPING.get(usize_index(match_number)?)?.0)
    } else if playoff_type == PlayoffType::DoubleElim4Team.as_number() {
        Some(DOUBLE_ELIM_4_MAPPING.get(usize_index(match_number)?)?.0)
    } else if playoff_type == PlayoffType::LegacyDoubleElim8Team.as_number() {
        Some(
            LEGACY_DOUBLE_ELIM_MAPPING
                .get(usize_index(match_number)?)?
                .0,
        )
    } else if playoff_type == PlayoffType::Bo3Finals.as_number()
        || playoff_type == PlayoffType::Bo5Finals.as_number()
    {
        Some(CompLevel::F)
    } else {
        let mut match_number = match_number;
        if playoff_type == PlayoffType::Bracket16Team.as_number() {
            return Some(get_elim_comp_level_octo(match_number));
        } else if playoff_type == PlayoffType::Bracket4Team.as_number() && match_number <= 12.0 {
            match_number += 12.0;
        } else if playoff_type == PlayoffType::Bracket2Team.as_number() && match_number <= 18.0 {
            match_number += 18.0;
        }
        if match_number <= 12.0 {
            Some(CompLevel::Qf)
        } else if match_number <= 18.0 {
            Some(CompLevel::Sf)
        } else {
            Some(CompLevel::F)
        }
    }
}

fn get_elim_comp_level_octo(match_number: f64) -> CompLevel {
    if match_number <= 24.0 {
        CompLevel::Ef
    } else if match_number <= 36.0 {
        CompLevel::Qf
    } else if match_number <= 42.0 {
        CompLevel::Sf
    } else {
        CompLevel::F
    }
}

fn get_elim_match_numbers(
    playoff_type: f64,
    comp_level: CompLevel,
    match_number: f64,
) -> Option<(f64, f64)> {
    if playoff_type == PlayoffType::AvgScore8Team.as_number() {
        if comp_level == CompLevel::Sf {
            Some((1.0, match_number - 8.0))
        } else if comp_level == CompLevel::F {
            Some((1.0, match_number - 14.0))
        } else {
            Some((1.0, match_number))
        }
    } else if playoff_type == PlayoffType::RoundRobin6Team.as_number() {
        let match_number = if match_number <= 15.0 {
            match_number
        } else {
            match_number - 15.0
        };
        Some((1.0, match_number))
    } else if playoff_type == PlayoffType::DoubleElim8Team.as_number() {
        let (_, set, m) = *DOUBLE_ELIM_MAPPING.get(usize_index(match_number)?)?;
        Some((f64::from(set), f64::from(m)))
    } else if playoff_type == PlayoffType::DoubleElim4Team.as_number() {
        let (_, set, m) = *DOUBLE_ELIM_4_MAPPING.get(usize_index(match_number)?)?;
        Some((f64::from(set), f64::from(m)))
    } else if playoff_type == PlayoffType::LegacyDoubleElim8Team.as_number() {
        let (_, set, m) = *LEGACY_DOUBLE_ELIM_MAPPING.get(usize_index(match_number)?)?;
        Some((f64::from(set), f64::from(m)))
    } else if playoff_type == PlayoffType::Bo3Finals.as_number()
        || playoff_type == PlayoffType::Bo5Finals.as_number()
    {
        Some((1.0, match_number))
    } else {
        let mut match_number = match_number;
        if playoff_type == PlayoffType::Bracket4Team.as_number() && match_number <= 12.0 {
            match_number += 12.0;
        } else if playoff_type == PlayoffType::Bracket2Team.as_number() && match_number <= 18.0 {
            match_number += 18.0;
        }
        let index = usize_index(match_number)?;
        let table = if playoff_type == PlayoffType::Bracket16Team.as_number() {
            &BRACKET_OCTO_ELIM_MAPPING[..]
        } else {
            &BRACKET_ELIM_MAPPING[..]
        };
        table
            .get(index)
            .map(|&(set, m)| (f64::from(set), f64::from(m)))
    }
}

/// Mapping keys are the positive integers 1..=N; every other key misses in JS.
fn usize_index(match_number: f64) -> Option<usize> {
    (match_number >= 1.0 && match_number.fract() == 0.0).then(|| match_number as usize - 1)
}

const BRACKET_ELIM_MAPPING: [(u32, u32); 24] = [
    (1, 1),
    (2, 1),
    (3, 1),
    (4, 1),
    (1, 2),
    (2, 2),
    (3, 2),
    (4, 2),
    (1, 3),
    (2, 3),
    (3, 3),
    (4, 3),
    (1, 1),
    (2, 1),
    (1, 2),
    (2, 2),
    (1, 3),
    (2, 3),
    (1, 1),
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
];

const BRACKET_OCTO_ELIM_MAPPING: [(u32, u32); 48] = [
    (1, 1),
    (2, 1),
    (3, 1),
    (4, 1),
    (5, 1),
    (6, 1),
    (7, 1),
    (8, 1),
    (1, 2),
    (2, 2),
    (3, 2),
    (4, 2),
    (5, 2),
    (6, 2),
    (7, 2),
    (8, 2),
    (1, 3),
    (2, 3),
    (3, 3),
    (4, 3),
    (5, 3),
    (6, 3),
    (7, 3),
    (8, 3),
    (1, 1),
    (2, 1),
    (3, 1),
    (4, 1),
    (1, 2),
    (2, 2),
    (3, 2),
    (4, 2),
    (1, 3),
    (2, 3),
    (3, 3),
    (4, 3),
    (1, 1),
    (2, 1),
    (1, 2),
    (2, 2),
    (1, 3),
    (2, 3),
    (1, 1),
    (1, 2),
    (1, 3),
    (1, 4),
    (1, 5),
    (1, 6),
];

const LEGACY_DOUBLE_ELIM_MAPPING: [(CompLevel, u32, u32); 15] = [
    (CompLevel::Ef, 1, 1),
    (CompLevel::Ef, 2, 1),
    (CompLevel::Ef, 3, 1),
    (CompLevel::Ef, 4, 1),
    (CompLevel::Ef, 5, 1),
    (CompLevel::Ef, 6, 1),
    (CompLevel::Qf, 1, 1),
    (CompLevel::Qf, 2, 1),
    (CompLevel::Qf, 3, 1),
    (CompLevel::Qf, 4, 1),
    (CompLevel::Sf, 1, 1),
    (CompLevel::Sf, 2, 1),
    (CompLevel::F, 1, 1),
    (CompLevel::F, 2, 1),
    (CompLevel::F, 2, 2),
];

const DOUBLE_ELIM_MAPPING: [(CompLevel, u32, u32); 19] = [
    (CompLevel::Sf, 1, 1),
    (CompLevel::Sf, 2, 1),
    (CompLevel::Sf, 3, 1),
    (CompLevel::Sf, 4, 1),
    (CompLevel::Sf, 5, 1),
    (CompLevel::Sf, 6, 1),
    (CompLevel::Sf, 7, 1),
    (CompLevel::Sf, 8, 1),
    (CompLevel::Sf, 9, 1),
    (CompLevel::Sf, 10, 1),
    (CompLevel::Sf, 11, 1),
    (CompLevel::Sf, 12, 1),
    (CompLevel::Sf, 13, 1),
    (CompLevel::F, 1, 1),
    (CompLevel::F, 1, 2),
    (CompLevel::F, 1, 3),
    (CompLevel::F, 1, 4),
    (CompLevel::F, 1, 5),
    (CompLevel::F, 1, 6),
];

const DOUBLE_ELIM_4_MAPPING: [(CompLevel, u32, u32); 11] = [
    (CompLevel::Sf, 1, 1),
    (CompLevel::Sf, 2, 1),
    (CompLevel::Sf, 3, 1),
    (CompLevel::Sf, 4, 1),
    (CompLevel::Sf, 5, 1),
    (CompLevel::F, 1, 1),
    (CompLevel::F, 1, 2),
    (CompLevel::F, 1, 3),
    (CompLevel::F, 1, 4),
    (CompLevel::F, 1, 5),
    (CompLevel::F, 1, 6),
];
