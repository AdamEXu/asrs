//! Wall-clock time, `Math.random` and local time zone handling. Clocks, random
//! streams and zones are injectable so the harness can drive both sides with
//! the same values (PLAN section 1.1 rule 3).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::math;
use crate::number;

/// `msPerDay`.
pub const MS_PER_DAY: f64 = 86_400_000.0;

/// The largest magnitude `TimeClip` accepts; beyond it a `Date` is `NaN`.
pub const MAX_TIME: f64 = 8.64e15;

/// `Date.now()`: integer milliseconds since the Unix epoch.
pub trait Clock {
    fn now(&self) -> f64;
}

/// The system wall clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> f64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis() as f64,
            Err(e) => -math::ceil(e.duration().as_secs_f64() * 1000.0),
        }
    }
}

/// A scripted clock returning a fixed value.
pub struct SetClock(pub f64);

impl Clock for SetClock {
    fn now(&self) -> f64 {
        self.0
    }
}

/// A `Math.random` stream: doubles in `[0, 1)`.
pub trait Rng {
    fn next_f64(&mut self) -> f64;
}

/// V8's `Math.random`: xorshift128+ over two u64 lanes, the low 52 bits of the
/// sum loaded into the mantissa of a `[1, 2)` double.
pub struct XorShift128Plus {
    s0: u64,
    s1: u64,
}

impl XorShift128Plus {
    pub fn new(s0: u64, s1: u64) -> Self {
        Self { s0, s1 }
    }

    /// Seeds 16 bytes from the OS entropy source, like upstream's
    /// `RandomSeedSource`.
    pub fn from_entropy() -> Self {
        let mut seed = [0u8; 16];
        if getrandom::fill(&mut seed).is_err() {
            let d = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            seed[..8].copy_from_slice(&d.as_nanos().to_le_bytes()[..8]);
            seed[8..].copy_from_slice(&(d.as_nanos() >> 64).to_le_bytes()[..8]);
        }
        Self::new(
            u64::from_le_bytes(seed[..8].try_into().expect("8 bytes")),
            u64::from_le_bytes(seed[8..].try_into().expect("8 bytes")),
        )
    }
}

impl Rng for XorShift128Plus {
    fn next_f64(&mut self) -> f64 {
        let mut s1 = self.s0;
        let s0 = self.s1;
        self.s0 = s0;
        s1 ^= s1 << 23;
        s1 ^= s1 >> 17;
        s1 ^= s0;
        s1 ^= s0 >> 26;
        self.s1 = s1;
        let random = self.s0.wrapping_add(self.s1);
        const MANTISSA_MASK: u64 = 0x000f_ffff_ffff_ffff;
        const EXPONENT_BITS: u64 = 0x3ff0_0000_0000_0000;
        f64::from_bits((random & MANTISSA_MASK) | EXPONENT_BITS) - 1.0
    }
}

/// A scripted stream yielding its values in order; once exhausted it yields
/// `NaN`, which flows through `floor(random * n)` to index 0 the way
/// `undefined` does in JS.
pub struct SetRng {
    values: Vec<f64>,
    index: usize,
}

impl SetRng {
    pub fn new(values: Vec<f64>) -> Self {
        Self { values, index: 0 }
    }
}

impl Rng for SetRng {
    fn next_f64(&mut self) -> f64 {
        let v = self.values.get(self.index).copied().unwrap_or(f64::NAN);
        self.index += 1;
        v
    }
}

/// What a zone reports about one instant: the wall-clock offset plus the
/// details `Date.prototype.toString` prints.
#[derive(Clone, Debug)]
pub struct ZoneInstant {
    /// Milliseconds to add to an epoch reading to get the wall clock.
    pub offset_ms: f64,
    /// ICU's daylight-saving flag, used to pick the long zone name.
    pub is_dst: bool,
    /// The tzdb abbreviation ("PST"); `None` for fixed zones.
    pub abbreviation: Option<String>,
}

/// A local time zone, backing ECMAScript's `LocalTZA` and `LocalTime`.
pub trait LocalZone {
    /// Milliseconds to add to an epoch reading to get the wall clock, or `NaN`
    /// when the instant is outside the representable range.
    fn offset_ms(&self, epoch_ms: f64) -> f64;

    /// The full instant record, or `None` when it is not representable.
    fn instant(&self, epoch_ms: f64) -> Option<ZoneInstant>;

    /// The `en-US` name `Date.prototype.toString` prints in parentheses.
    fn long_name(&self, instant: &ZoneInstant) -> String;

    /// `LocalTZA(t, isUTC=false)` with ECMAScript's `compatible` disambiguation:
    /// a wall-clock reading in a fold resolves to the earlier instant and a
    /// reading in a gap resolves with the offset in effect before the jump.
    /// `NaN` when the result is not representable.
    fn local_to_epoch(&self, local_ms: f64) -> f64;
}

/// A fixed offset zone, for scripted tests and UTC (`FixedZone::UTC`).
#[derive(Clone, Copy)]
pub struct FixedZone(pub f64);

impl FixedZone {
    pub const UTC: FixedZone = FixedZone(0.0);
}

impl LocalZone for FixedZone {
    fn offset_ms(&self, _epoch_ms: f64) -> f64 {
        self.0
    }

    fn instant(&self, epoch_ms: f64) -> Option<ZoneInstant> {
        if epoch_ms.is_finite() {
            Some(ZoneInstant {
                offset_ms: self.0,
                is_dst: false,
                abbreviation: None,
            })
        } else {
            None
        }
    }

    fn long_name(&self, instant: &ZoneInstant) -> String {
        gmt_offset(instant.offset_ms)
    }

    fn local_to_epoch(&self, local_ms: f64) -> f64 {
        local_ms - self.0
    }
}

/// Days in 400 Gregorian years: shifting an instant by whole cycles keeps the
/// calendar and any recurring DST rules aligned, which is how instants beyond
/// jiff's civil range (year -9999..=9999) are evaluated. Before the tzdb's
/// first transition the offset is constant, so below-range instants clamp to
/// the earliest representable timestamp instead.
const CYCLE_DAYS: i64 = 146_097;
const CYCLE_MS: i64 = CYCLE_DAYS * 86_400_000;

/// A tzdb zone (IANA name, POSIX rule or the system zone), resolved through
/// `jiff`. Its civil range is year -9999..=9999; outside it the compatible
/// extrapolation above applies.
#[derive(Clone)]
pub struct NamedZone(jiff::tz::TimeZone);

impl NamedZone {
    pub fn get(name: &str) -> Result<Self, jiff::Error> {
        jiff::tz::TimeZone::get(name).map(Self)
    }

    /// The host's zone, what `TZ`-less `Date` getters mean in the app.
    pub fn system() -> Self {
        Self(jiff::tz::TimeZone::system())
    }

    pub fn iana_name(&self) -> Option<&str> {
        self.0.iana_name()
    }

    /// `Intl.DateTimeFormat().resolvedOptions().timeZone` under `en-US`: the
    /// CLDR canonical name, which keeps some legacy aliases.
    pub fn resolved_name(&self) -> String {
        match self.iana_name() {
            Some("Asia/Kolkata") => "Asia/Calcutta".to_owned(),
            Some(name) => name.to_owned(),
            None => "GMT".to_owned(),
        }
    }

    /// An epoch reading inside jiff's timestamp range, shifted backward by
    /// whole Gregorian cycles when the instant is past the last representable
    /// year, or clamped to the earliest timestamp when it precedes the tzdb's
    /// first transition (where the offset is constant).
    fn representable(&self, epoch_ms: i64) -> Option<jiff::Timestamp> {
        let days = epoch_ms.div_euclid(86_400_000);
        let (year, _, _) = civil_from_days(days);
        if year > 9999 {
            let cycles = (year - 9999 + 399) / 400;
            let shifted = epoch_ms.checked_sub(cycles * CYCLE_MS)?;
            jiff::Timestamp::from_millisecond(shifted).ok()
        } else if year < -9999 {
            Some(jiff::Timestamp::MIN)
        } else {
            jiff::Timestamp::from_millisecond(epoch_ms).ok()
        }
    }
}

impl LocalZone for NamedZone {
    fn offset_ms(&self, epoch_ms: f64) -> f64 {
        self.instant(epoch_ms).map_or(f64::NAN, |i| i.offset_ms)
    }

    fn instant(&self, epoch_ms: f64) -> Option<ZoneInstant> {
        let ms = whole_millis(epoch_ms)?;
        let ts = self.representable(ms)?;
        let info = self.0.to_offset_info(ts);
        Some(ZoneInstant {
            offset_ms: f64::from(info.offset().seconds()) * 1000.0,
            is_dst: info.dst().is_dst(),
            abbreviation: Some(info.abbreviation().to_owned()),
        })
    }

    fn long_name(&self, instant: &ZoneInstant) -> String {
        let name = match (self.iana_name(), instant.is_dst) {
            (Some("UTC"), _) => "Coordinated Universal Time",
            (Some("America/Los_Angeles"), false) => "Pacific Standard Time",
            (Some("America/Los_Angeles"), true) => "Pacific Daylight Time",
            (Some("Asia/Kolkata"), _) => "India Standard Time",
            (Some("Pacific/Auckland"), false) => "New Zealand Standard Time",
            (Some("Pacific/Auckland"), true) => "New Zealand Daylight Time",
            _ => {
                return instant
                    .abbreviation
                    .clone()
                    .unwrap_or_else(|| "GMT".to_owned())
            }
        };
        name.to_owned()
    }

    fn local_to_epoch(&self, local_ms: f64) -> f64 {
        let Some(ms) = whole_millis(local_ms) else {
            return f64::NAN;
        };
        let days = ms.div_euclid(86_400_000);
        let (year, _, _) = civil_from_days(days);
        if year < -9999 {
            let offset = self
                .0
                .to_offset_info(jiff::Timestamp::MIN)
                .offset()
                .seconds();
            return ms as f64 - f64::from(offset) * 1000.0;
        }
        let cycles = if year > 9999 { (year - 9999 + 399) / 400 } else { 0 };
        let shifted = ms - cycles * CYCLE_MS;
        let Some(dt) = civil_datetime(shifted) else {
            return f64::NAN;
        };
        match self.0.to_zoned_datetime(dt) {
            Ok(zoned) => zoned.timestamp().as_millisecond() as f64 + (cycles * CYCLE_MS) as f64,
            Err(_) => f64::NAN,
        }
    }
}

fn whole_millis(ms: f64) -> Option<i64> {
    if ms.is_finite() && ms.abs() <= 9.0e18 {
        Some(ms.trunc() as i64)
    } else {
        None
    }
}

fn civil_datetime(local_ms: i64) -> Option<jiff::civil::DateTime> {
    let days = local_ms.div_euclid(86_400_000);
    let day_ms = local_ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second, milli) = time_from_day_ms(day_ms);
    jiff::civil::DateTime::new(
        year.try_into().ok()?,
        month as i8,
        day as i8,
        hour as i8,
        minute as i8,
        second as i8,
        (milli * 1_000_000) as i32,
    )
    .ok()
}

impl<T: LocalZone + ?Sized> LocalZone for &T {
    fn offset_ms(&self, epoch_ms: f64) -> f64 {
        (**self).offset_ms(epoch_ms)
    }

    fn instant(&self, epoch_ms: f64) -> Option<ZoneInstant> {
        (**self).instant(epoch_ms)
    }

    fn long_name(&self, instant: &ZoneInstant) -> String {
        (**self).long_name(instant)
    }

    fn local_to_epoch(&self, local_ms: f64) -> f64 {
        (**self).local_to_epoch(local_ms)
    }
}

/// Wall-clock fields of an epoch reading in `zone`. `None` is JS's invalid
/// `Date` (`NaN` or beyond `TimeClip`'s range).
pub fn epoch_to_civil(epoch_ms: f64, zone: &impl LocalZone) -> Option<CivilTime> {
    let clipped = time_clip(epoch_ms);
    if clipped.is_nan() {
        return None;
    }
    let local = clipped + zone.offset_ms(clipped);
    if !local.is_finite() {
        return None;
    }
    let days = math::floor(local / MS_PER_DAY);
    let (year, month, day) = civil_from_days(days as i64);
    let day_ms = local - days * MS_PER_DAY;
    let (hour, minute, second, milli) = time_from_day_ms(day_ms as i64);
    Some(CivilTime {
        year: year as i32,
        month: month as u32,
        day: day as u32,
        hour: hour as u32,
        minute: minute as u32,
        second: second as u32,
        millisecond: milli as u32,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millisecond: u32,
}

/// ECMAScript `TimeClip`: truncate to integer, reject non-finite values and
/// magnitudes over `MAX_TIME`, canonicalize -0 to +0.
pub fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > MAX_TIME {
        f64::NAN
    } else {
        math::trunc(t) + 0.0
    }
}

/// `shared/util.ts formatDate`: `YYYY-MM-DD` in the zone's wall clock.
/// An invalid `Date` gives `NaN-NaN-NaN`.
pub fn format_date(epoch_ms: f64, zone: &impl LocalZone) -> String {
    let Some(c) = epoch_to_civil(epoch_ms, zone) else {
        return "NaN-NaN-NaN".to_owned();
    };
    format!(
        "{}-{:02}-{:02}",
        number::to_string(c.year as f64),
        c.month,
        c.day
    )
}

/// `shared/util.ts formatTimeWithMS`: `seconds.mmm`.
pub fn format_time_with_ms(time: f64) -> String {
    let seconds = math::floor(time);
    let milliseconds = math::floor((time - seconds) * 1000.0);
    let ms = crate::string::JsString::from(number::to_string(milliseconds));
    let padded = ms
        .pad_start(3.0, Some(&crate::string::JsString::from("0")))
        .unwrap_or_else(|_| crate::string::JsString::new());
    format!("{}.{}", number::to_string(seconds), padded.to_string_lossy())
}

/// `Date.prototype.toLocaleTimeString()` with the locale fixed to `en-US`:
/// `h:mm:ss AM/PM` in the zone's wall clock. Invalid date gives `Invalid Date`.
pub fn to_locale_time_string(epoch_ms: f64, zone: &impl LocalZone) -> String {
    let Some(c) = epoch_to_civil(epoch_ms, zone) else {
        return "Invalid Date".to_owned();
    };
    let hour12 = match c.hour % 12 {
        0 => 12,
        h => h,
    };
    let suffix = if c.hour < 12 { "AM" } else { "PM" };
    format!("{hour12}:{:02}:{:02} {suffix}", c.minute, c.second)
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The `GMT±HHmm` text of an offset; seconds are dropped the way V8 drops
/// them in historical sub-minute offsets (`GMT-0752` for LMT `-7:52:58`).
fn gmt_offset(offset_ms: f64) -> String {
    let sign = if offset_ms < 0.0 { '-' } else { '+' };
    let magnitude = (offset_ms / 1000.0).abs() as u64;
    format!("GMT{sign}{:02}{:02}", magnitude / 3600, magnitude % 3600 / 60)
}

/// `Date.prototype.toString()` under `en-US`:
/// `Www Mmm DD YYYY HH:mm:ss GMT±HHmm (zone name)`.
pub fn date_to_string(epoch_ms: f64, zone: &impl LocalZone) -> String {
    let clipped = time_clip(epoch_ms);
    let Some(instant) = zone.instant(clipped) else {
        return "Invalid Date".to_owned();
    };
    let Some(c) = epoch_to_civil(clipped, zone) else {
        return "Invalid Date".to_owned();
    };
    let days = (clipped as i64).div_euclid(86_400_000);
    let weekday = WEEKDAYS[(days + 4).rem_euclid(7) as usize];
    let year = if c.year < 0 {
        format!("-{:04}", c.year.unsigned_abs())
    } else {
        format!("{:04}", c.year)
    };
    format!(
        "{weekday} {} {:02} {} {:02}:{:02}:{:02} {} ({})",
        MONTHS[(c.month - 1) as usize],
        c.day,
        year,
        c.hour,
        c.minute,
        c.second,
        gmt_offset(instant.offset_ms),
        zone.long_name(&instant)
    )
}

/// `Date.prototype.getTimezoneOffset()`: UTC minus local, in whole minutes
/// truncated toward zero (V8's getter drops sub-minute history like LMT).
pub fn get_timezone_offset(epoch_ms: f64, zone: &impl LocalZone) -> f64 {
    let clipped = time_clip(epoch_ms);
    if clipped.is_nan() {
        return f64::NAN;
    }
    math::trunc(-zone.offset_ms(clipped) / 60_000.0) + 0.0
}

/// `new Date(year, month?, date?, hours?, minutes?, seconds?, ms?)`: the local
/// wall-clock reading converted to epoch ms under `zone`. `fields` holds the
/// `ToNumber` results of the arguments, starting at `year`; later fields
/// default like the constructor (`date` to 1, the rest to 0). V8 truncates
/// every field toward zero and maps years 0..=99 to 1900..=1999.
pub fn local_date(fields: &[f64], zone: &impl LocalZone) -> f64 {
    let field = |i: usize, default: f64| fields.get(i).copied().unwrap_or(default);
    let y = field(0, f64::NAN);
    if y.is_nan() {
        return f64::NAN;
    }
    let mut fields_f = [0.0; 7];
    fields_f[0] = number::to_integer_or_infinity(y);
    for (i, slot) in fields_f.iter_mut().enumerate().skip(1) {
        let raw = field(i, if i == 2 { 1.0 } else { 0.0 });
        if raw.is_nan() {
            return f64::NAN;
        }
        *slot = number::to_integer_or_infinity(raw);
    }
    let [year, month, date, hour, minute, second, milli] = fields_f;
    if !(year.is_finite()
        && month.is_finite()
        && date.is_finite()
        && hour.is_finite()
        && minute.is_finite()
        && second.is_finite()
        && milli.is_finite())
    {
        return f64::NAN;
    }
    let year = if (0.0..=99.0).contains(&year) {
        year + 1900.0
    } else {
        year
    };
    // MakeDay: ym = year + floor(month / 12), mn = month mod 12 (Euclidean),
    // then the Gregorian day number of (ym, mn + 1, date).
    let ym = year + math::floor(month / 12.0);
    let mn = month.rem_euclid(12.0);
    if !ym.is_finite() {
        return f64::NAN;
    }
    let days = days_from_civil(ym, mn + 1.0, 1.0) + (date - 1.0);
    let time = hour * 3_600_000.0 + minute * 60_000.0 + second * 1000.0 + milli;
    let local_ms = days * MS_PER_DAY + time;
    if !local_ms.is_finite() {
        return f64::NAN;
    }
    time_clip(zone.local_to_epoch(local_ms))
}

/// `shared/util.ts createUUID`: 32 draws of `floor(random * 36)` into
/// `abcdefghijklmnopqrstuvwxyz0123456789`, `charAt` semantics included.
pub fn create_uuid(rng: &mut dyn Rng) -> String {
    const OPTIONS: &str = "abcdefghijklmnopqrstuvwxyz0123456789";
    let options = crate::string::JsString::from(OPTIONS);
    let mut out = crate::string::JsString::new();
    for _ in 0..32 {
        let index = math::floor(rng.next_f64() * OPTIONS.len() as f64);
        out.push_str(&options.char_at(index));
    }
    out.to_string_lossy()
}

/// Days from the epoch to `y-m(1-12)-d` on the proleptic Gregorian calendar,
/// the same calendar ECMAScript's `Day`/`MakeDay` uses.
fn days_from_civil(y: f64, m: f64, d: f64) -> f64 {
    let y = if m <= 2.0 { y - 1.0 } else { y };
    let era = math::floor(y / 400.0);
    let yoe = y - era * 400.0;
    let doy = math::floor((153.0 * (if m > 2.0 { m - 3.0 } else { m + 9.0 }) + 2.0) / 5.0)
        + d
        - 1.0;
    let doe = yoe * 365.0 + math::floor(yoe / 4.0) - math::floor(yoe / 100.0) + doy;
    era * 146_097.0 + doe - 719_468.0
}

/// `(year, month 1-12, day)` of `z` days since the epoch, the inverse of
/// `days_from_civil`. `z` is an integer-valued `i64` (days are small).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `(hour, minute, second, millisecond)` of `ms` milliseconds into a day;
/// `ms` is in `[0, 86_400_000)`.
fn time_from_day_ms(ms: i64) -> (i64, i64, i64, i64) {
    let hour = ms / 3_600_000;
    let minute = (ms % 3_600_000) / 60_000;
    let second = (ms % 60_000) / 1000;
    let milli = ms % 1000;
    (hour, minute, second, milli)
}
