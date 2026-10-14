//! Calendar arithmetic and the text forms of dates, times and durations.
//!
//! Dates are days since 1970-01-01 in the proleptic Gregorian calendar, years 1 to 9999. Times
//! and durations count ticks of 100 nanoseconds.

use crate::culture::{Culture, DateOrder};

pub const TICKS_PER_SECOND: i64 = 10_000_000;
pub const TICKS_PER_MINUTE: i64 = 60 * TICKS_PER_SECOND;
pub const TICKS_PER_HOUR: i64 = 60 * TICKS_PER_MINUTE;
pub const TICKS_PER_DAY: i64 = 24 * TICKS_PER_HOUR;
/// 0001-01-01 and 9999-12-31 as days since 1970-01-01.
pub const MIN_DAY: i64 = -719_162;
pub const MAX_DAY: i64 = 2_932_896;
/// Excel's day 0 (1899-12-30) as days since 1970-01-01: what `Number.From(date)` counts from.
pub const OLE_EPOCH_DAY: i64 = -25_569;

pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => 28 + is_leap(y) as i64,
    }
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// The day number of a valid date in years 1 to 9999.
pub fn date(y: i64, m: i64, d: i64) -> Option<i64> {
    ((1..=9999).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m)).then(|| days_from_civil(y, m, d))
}

pub fn valid_day(days: i64) -> bool {
    (MIN_DAY..=MAX_DAY).contains(&days)
}

/// Ticks since midnight for a valid time of day.
pub fn time(h: i64, m: i64, s: f64) -> Option<i64> {
    let ok = ((0..24).contains(&h) && (0..60).contains(&m) && (0.0..60.0).contains(&s)) || (h == 24 && m == 0 && s == 0.0);
    ok.then(|| h * TICKS_PER_HOUR + m * TICKS_PER_MINUTE + (s * TICKS_PER_SECOND as f64).round() as i64)
}

/// Day of the week, Sunday = 0.
pub fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

/// Whole days and ticks into the day.
pub fn split_ticks(t: i64) -> (i64, i64) {
    (t.div_euclid(TICKS_PER_DAY), t.rem_euclid(TICKS_PER_DAY))
}

pub fn join_ticks(days: i64, tod: i64) -> i64 {
    days.saturating_mul(TICKS_PER_DAY).saturating_add(tod)
}

/// Hours, minutes and seconds (with fraction) of a time of day.
pub fn time_parts(tod: i64) -> (i64, i64, f64) {
    let tod = tod.rem_euclid(TICKS_PER_DAY);
    (tod / TICKS_PER_HOUR, (tod % TICKS_PER_HOUR) / TICKS_PER_MINUTE, (tod % TICKS_PER_MINUTE) as f64 / TICKS_PER_SECOND as f64)
}

/// Days, hours, minutes and seconds of a duration; all carry the sign of the duration.
pub fn duration_parts(t: i64) -> (i64, i64, i64, f64) {
    (
        t / TICKS_PER_DAY,
        (t % TICKS_PER_DAY) / TICKS_PER_HOUR,
        (t % TICKS_PER_HOUR) / TICKS_PER_MINUTE,
        (t % TICKS_PER_MINUTE) as f64 / TICKS_PER_SECOND as f64,
    )
}

pub fn utc_ticks(local: i64, offset_minutes: i32) -> i64 {
    local.saturating_sub(offset_minutes as i64 * TICKS_PER_MINUTE)
}

/// `days` moved by `months`, the day of the month clamped to the target month's length.
pub fn add_months(days: i64, months: i64) -> Option<i64> {
    let (y, m, d) = civil_from_days(days);
    let total = y.checked_mul(12)?.checked_add(m - 1)?.checked_add(months)?;
    let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    date(ny, nm, d.min(days_in_month(ny, nm)))
}

/// Day of the year, 1-based.
pub fn day_of_year(days: i64) -> i64 {
    let (y, _, _) = civil_from_days(days);
    days - days_from_civil(y, 1, 1) + 1
}

// ---------------------------------------------------------------- formatting

/// The parts a format pattern can show.
#[derive(Clone, Copy, Default)]
pub struct Moment {
    pub days: i64,
    pub tod: i64,
    pub offset: Option<i32>,
}

fn push_num(out: &mut String, n: i64, width: usize) {
    use std::fmt::Write as _;
    let _ = write!(out, "{n:0width$}");
}

fn offset_text(off: i32, colon: bool, minutes: bool) -> String {
    let sign = if off < 0 { '-' } else { '+' };
    let (h, m) = (off.abs() / 60, off.abs() % 60);
    match (minutes, colon) {
        (false, _) => format!("{sign}{h:02}"),
        (true, true) => format!("{sign}{h:02}:{m:02}"),
        (true, false) => format!("{sign}{h:02}{m:02}"),
    }
}

/// Expands a one-letter standard format into its pattern.
fn standard_pattern(f: char, c: &Culture) -> Option<String> {
    Some(match f {
        'd' => c.short_date.to_string(),
        'D' => c.long_date.to_string(),
        't' => c.short_time.to_string(),
        'T' => c.long_time.to_string(),
        'g' => format!("{} {}", c.short_date, c.short_time),
        'G' => format!("{} {}", c.short_date, c.long_time),
        'f' => format!("{} {}", c.long_date, c.short_time),
        'F' => format!("{} {}", c.long_date, c.long_time),
        's' => "yyyy'-'MM'-'dd'T'HH':'mm':'ss".to_string(),
        'u' => "yyyy'-'MM'-'dd HH':'mm':'ss'Z'".to_string(),
        'o' | 'O' => "yyyy'-'MM'-'dd'T'HH':'mm':'ss'.'fffffffK".to_string(),
        'M' | 'm' => "MMMM d".to_string(),
        'Y' | 'y' => "MMMM yyyy".to_string(),
        _ => return None,
    })
}

/// Formats with a .NET-style pattern: `yyyy-MM-dd`, `dd/MM/yy HH:mm`, `MMMM`, `ddd`, or a
/// one-letter standard format (`d`, `D`, `t`, `T`, `g`, `G`, `s`, `o`).
pub fn format(m: Moment, pattern: &str, c: &Culture) -> String {
    let mut chars = pattern.chars();
    let expanded;
    let pattern = match (chars.next(), chars.next()) {
        (Some(f), None) => match standard_pattern(f, c) {
            Some(p) => {
                expanded = p;
                expanded.as_str()
            }
            None => pattern,
        },
        (Some('%'), Some(_)) => pattern.get(1..).unwrap_or(pattern),
        _ => pattern,
    };
    let (y, mo, d) = civil_from_days(m.days);
    let tod = m.tod.rem_euclid(TICKS_PER_DAY);
    let (h, mi) = (tod / TICKS_PER_HOUR, (tod % TICKS_PER_HOUR) / TICKS_PER_MINUTE);
    let sec = (tod % TICKS_PER_MINUTE) / TICKS_PER_SECOND;
    let frac = tod % TICKS_PER_SECOND;
    let mut out = String::new();
    let cs: Vec<char> = pattern.chars().take(1024).collect();
    let mut i = 0;
    while let Some(&ch) = cs.get(i) {
        let mut run = 1;
        while cs.get(i + run) == Some(&ch) {
            run += 1;
        }
        match ch {
            'y' => match run {
                1 => push_num(&mut out, y % 100, 1),
                2 => push_num(&mut out, y % 100, 2),
                n => push_num(&mut out, y, n),
            },
            'M' => match run {
                1 => push_num(&mut out, mo, 1),
                2 => push_num(&mut out, mo, 2),
                3 => out.push_str(&c.month_short((mo - 1) as usize)),
                _ => out.push_str(c.months.get((mo - 1) as usize).copied().unwrap_or("")),
            },
            'd' => match run {
                1 => push_num(&mut out, d, 1),
                2 => push_num(&mut out, d, 2),
                3 => out.push_str(&c.day_short(weekday(m.days) as usize)),
                _ => out.push_str(c.days.get(weekday(m.days) as usize).copied().unwrap_or("")),
            },
            'H' => push_num(&mut out, h, run.min(2)),
            'h' => push_num(&mut out, if h % 12 == 0 { 12 } else { h % 12 }, run.min(2)),
            'm' => push_num(&mut out, mi, run.min(2)),
            's' => push_num(&mut out, sec, run.min(2)),
            'f' | 'F' => {
                let digits = format!("{frac:07}");
                let shown = digits.get(..run.min(7)).unwrap_or("");
                if ch == 'f' {
                    out.push_str(shown);
                } else {
                    let trimmed = shown.trim_end_matches('0');
                    // An empty fraction also drops the decimal point written just before it.
                    if trimmed.is_empty() && out.ends_with(['.', ',']) {
                        out.pop();
                    }
                    out.push_str(trimmed);
                }
            }
            't' => {
                let s = if h < 12 { c.am } else { c.pm };
                let s = if s.is_empty() { if h < 12 { "AM" } else { "PM" } } else { s };
                if run == 1 {
                    out.extend(s.chars().take(1));
                } else {
                    out.push_str(s);
                }
            }
            'z' => out.push_str(&offset_text(m.offset.unwrap_or(0), true, run >= 3)),
            'K' => {
                if let Some(off) = m.offset {
                    out.push_str(&offset_text(off, true, true));
                }
                run = 1;
            }
            '\'' | '"' => {
                let mut j = i + 1;
                while let Some(&q) = cs.get(j) {
                    if q == ch {
                        break;
                    }
                    out.push(q);
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            '\\' => {
                if let Some(&next) = cs.get(i + 1) {
                    out.push(next);
                }
                i += 2;
                continue;
            }
            other => {
                for _ in 0..run {
                    out.push(other);
                }
            }
        }
        i += run;
    }
    out
}

pub fn date_to_text(days: i64, c: &Culture) -> String {
    format(Moment { days, ..Default::default() }, c.short_date, c)
}

/// A time of day; seconds and their fraction show only when they aren't zero.
pub fn time_to_text(tod: i64, c: &Culture) -> String {
    let pattern = if tod % TICKS_PER_SECOND != 0 {
        c.long_time.replace("ss", "ss.FFFFFFF")
    } else if tod % TICKS_PER_MINUTE != 0 {
        c.long_time.to_string()
    } else {
        c.short_time.to_string()
    };
    format(Moment { tod, ..Default::default() }, &pattern, c)
}

pub fn datetime_to_text(ticks: i64, c: &Culture) -> String {
    let (days, tod) = split_ticks(ticks);
    let time = if tod % TICKS_PER_SECOND != 0 { c.long_time.replace("ss", "ss.FFFFFFF") } else { c.long_time.to_string() };
    format(Moment { days, tod, offset: None }, &format!("{} {}", c.short_date, time), c)
}

pub fn datetimezone_to_text(ticks: i64, offset: i32, c: &Culture) -> String {
    format!("{} {}", datetime_to_text(ticks, c), offset_text(offset, true, true))
}

/// `d.hh:mm:ss` with the day and the fraction only when they aren't zero.
pub fn duration_to_text(t: i64) -> String {
    let neg = t < 0;
    let a = t.unsigned_abs();
    let day = TICKS_PER_DAY as u64;
    let (d, rest) = (a / day, a % day);
    let (h, mi, s, f) =
        (rest / TICKS_PER_HOUR as u64, (rest / TICKS_PER_MINUTE as u64) % 60, (rest / TICKS_PER_SECOND as u64) % 60, rest % TICKS_PER_SECOND as u64);
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if d > 0 {
        out.push_str(&format!("{d}."));
    }
    out.push_str(&format!("{h:02}:{mi:02}:{s:02}"));
    if f > 0 {
        out.push_str(&format!(".{f:07}"));
    }
    out
}

// ---------------------------------------------------------------- parsing

fn digits(s: &str) -> Option<i64> {
    (!s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

fn expand_year(y: i64, len: usize) -> i64 {
    match len {
        1 | 2 => {
            if y < 50 {
                2000 + y
            } else {
                1900 + y
            }
        }
        _ => y,
    }
}

/// `H:mm`, `H:mm:ss`, `H:mm:ss.fffffff`, each optionally followed by AM or PM.
pub fn parse_time(text: &str) -> Option<i64> {
    let s = text.trim();
    let lower = s.to_lowercase();
    let (body, half) = if let Some(b) = lower.strip_suffix("am").or_else(|| lower.strip_suffix("a.m.")) {
        (b.trim_end(), Some(false))
    } else if let Some(b) = lower.strip_suffix("pm").or_else(|| lower.strip_suffix("p.m.")) {
        (b.trim_end(), Some(true))
    } else {
        (lower.as_str(), None)
    };
    let mut parts = body.split(':');
    let h = digits(parts.next()?.trim())?;
    let (mi, sec) = match (parts.next(), parts.next(), parts.next()) {
        (Some(m), None, _) => (digits(m)?, 0.0),
        (Some(m), Some(sec), None) => {
            let sec = sec.replace(',', ".");
            if sec.is_empty() || !sec.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
                return None;
            }
            (digits(m)?, sec.parse::<f64>().ok()?)
        }
        // "3 PM"
        (None, _, _) if half.is_some() => (0, 0.0),
        _ => return None,
    };
    let h = match half {
        Some(pm) => {
            if !(1..=12).contains(&h) {
                return None;
            }
            h % 12 + if pm { 12 } else { 0 }
        }
        None => h,
    };
    time(h, mi, sec)
}

/// A UTC offset: `Z`, `+hh:mm`, `-hhmm`, `+hh`. Minutes east of UTC.
fn parse_offset(s: &str) -> Option<i32> {
    if s.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let (sign, rest) = match s.chars().next()? {
        '+' => (1, s.get(1..)?),
        '-' | '\u{2212}' => (-1, s.get(s.chars().next()?.len_utf8()..)?),
        _ => return None,
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (digits(h)?, digits(m)?),
        None if rest.len() == 4 => (digits(rest.get(..2)?)?, digits(rest.get(2..)?)?),
        None => (digits(rest)?, 0),
    };
    (h <= 14 && m < 60).then_some(sign * (h * 60 + m) as i32)
}

fn month_from_name(word: &str, c: &Culture) -> Option<i64> {
    let w = word.trim_end_matches('.').to_lowercase();
    if w.chars().count() < 3 {
        return None;
    }
    for names in [&c.months, &Culture::find("en-US").months] {
        if let Some(i) = names.iter().position(|n| {
            let n = n.to_lowercase();
            n == w || (w.chars().count() >= 3 && n.starts_with(&w))
        }) {
            return Some(i as i64 + 1);
        }
    }
    None
}

/// Reads a date: ISO (`2024-01-31`), numeric in the culture's order (`31/01/2024`), or with a
/// month name (`31 January 2024`, `Jan 31, 2024`, `January 2024`).
pub fn parse_date(text: &str, c: &Culture) -> Option<i64> {
    let s = text.trim();
    if s.is_empty() || s.len() > 64 {
        return None;
    }
    let numeric: Vec<&str> = s.split(['/', '-', '.']).map(str::trim).collect();
    if numeric.len() == 3 && numeric.iter().all(|p| digits(p).is_some()) {
        let n: Vec<i64> = numeric.iter().filter_map(|p| digits(p)).collect();
        let (a, b, z) = (*n.first()?, *n.get(1)?, *n.get(2)?);
        let first_len = numeric.first()?.len();
        let last_len = numeric.get(2)?.len();
        let (y, m, d) = if first_len >= 3 {
            (a, b, z)
        } else {
            match c.order {
                DateOrder::Mdy => (expand_year(z, last_len), a, b),
                DateOrder::Dmy => (expand_year(z, last_len), b, a),
                DateOrder::Ymd => (expand_year(a, first_len), b, z),
            }
        };
        return date(y, m, d);
    }
    // yyyyMMdd
    if s.len() == 8 && digits(s).is_some() {
        return date(digits(s.get(..4)?)?, digits(s.get(4..6)?)?, digits(s.get(6..)?)?);
    }
    // Month names.
    let words: Vec<&str> = s.split([' ', ',', '-', '/', '\u{a0}']).filter(|w| !w.is_empty()).collect();
    let mut month = None;
    let mut nums: Vec<(i64, usize)> = Vec::new();
    for w in &words {
        if let Some(n) = digits(w.trim_end_matches(['.', 'º', 'ª'])) {
            nums.push((n, w.trim_end_matches(['.', 'º', 'ª']).len()));
        } else if let Some(m) = month_from_name(w, c) {
            if month.replace(m).is_some() {
                return None;
            }
        } else if !matches!(w.to_lowercase().as_str(), "de" | "of" | "the" | "del")
            && !c.days.iter().any(|d| d.eq_ignore_ascii_case(w.trim_end_matches(',')))
        {
            return None;
        }
    }
    let m = month?;
    match nums.as_slice() {
        [(y, len)] if *len >= 3 => date(*y, m, 1),
        [(a, la), (b, lb)] => {
            // The year is the long number, or the one that can't be a day.
            if *la >= 3 || *a > 31 { date(*a, m, *b) } else { date(expand_year(*b, *lb), m, *a) }
        }
        _ => None,
    }
}

/// Reads a date with an optional time and UTC offset. Returns ticks since 1970-01-01T00:00
/// (local to the offset), the offset if one was written, and whether a time was written.
pub fn parse_datetime(text: &str, c: &Culture) -> Option<(i64, Option<i32>, bool)> {
    let s = text.trim();
    if s.is_empty() || s.len() > 96 {
        return None;
    }
    // Split off a trailing offset.
    let (body, offset) = split_offset(s);
    // ISO: date 'T' time.
    if let Some((d, t)) = body.split_once(['T', 't'])
        && let (Some(days), Some(tod)) = (parse_date(d, c), parse_time(t))
    {
        return Some((join_ticks(days, tod), offset, true));
    }
    if let Some(days) = parse_date(body, c) {
        return Some((join_ticks(days, 0), offset, false));
    }
    // "date time": try each space as the boundary, from the right.
    for (i, _) in body.char_indices().rev().filter(|(_, ch)| ch.is_whitespace()) {
        let (d, t) = (body.get(..i)?, body.get(i..)?);
        if let (Some(days), Some(tod)) = (parse_date(d, c), parse_time(t)) {
            return Some((join_ticks(days, tod), offset, true));
        }
    }
    None
}

/// Splits a trailing `Z` or `±hh:mm` offset from date-time text.
fn split_offset(s: &str) -> (&str, Option<i32>) {
    if let Some(body) = s.strip_suffix(['Z', 'z'])
        && body.ends_with(|ch: char| ch.is_ascii_digit())
    {
        return (body, Some(0));
    }
    // An offset sign comes after a time, so after the last colon or 'T'.
    let time_at = s.rfind(['T', 't', ' ']).unwrap_or(0);
    if let Some(i) = s.get(time_at..).and_then(|tail| tail.rfind(['+', '-'])).map(|i| i + time_at) {
        let (body, off) = (s.get(..i).unwrap_or(s), s.get(i..).unwrap_or(""));
        if body.contains(':')
            && let Some(o) = parse_offset(off.trim())
        {
            return (body.trim_end(), Some(o));
        }
    }
    (s, None)
}

/// Reads a duration: `[-][d.]hh:mm[:ss[.fffffff]]`, a number of days, or ISO 8601 (`P1DT2H`).
pub fn parse_duration(text: &str) -> Option<i64> {
    let s = text.trim();
    if s.is_empty() || s.len() > 64 {
        return None;
    }
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let ticks = if let Some(iso) = s.strip_prefix(['P', 'p']) {
        parse_iso_duration(iso)?
    } else if !s.contains(':') {
        digits(s)?.checked_mul(TICKS_PER_DAY)?
    } else {
        let (days, clock) = match s.split_once('.') {
            Some((d, rest)) if rest.contains(':') && !d.contains(':') => (digits(d)?, rest),
            _ => (0, s),
        };
        let mut parts = clock.split(':');
        let h = digits(parts.next()?)?;
        let m = digits(parts.next()?)?;
        let sec: f64 = match parts.next() {
            Some(x) if !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit() || b == b'.') => x.parse().ok()?,
            Some(_) => return None,
            None => 0.0,
        };
        if parts.next().is_some() || h > 23 || m > 59 || sec >= 60.0 {
            return None;
        }
        days.checked_mul(TICKS_PER_DAY)? + h * TICKS_PER_HOUR + m * TICKS_PER_MINUTE + (sec * TICKS_PER_SECOND as f64).round() as i64
    };
    Some(if neg { -ticks } else { ticks })
}

fn parse_iso_duration(s: &str) -> Option<i64> {
    let (date, time) = match s.split_once(['T', 't']) {
        Some((d, t)) => (d, t),
        None => (s, ""),
    };
    let mut total = 0f64;
    for (part, units) in [
        (date, [('D', TICKS_PER_DAY), ('W', 7 * TICKS_PER_DAY), ('\0', 0)]),
        (time, [('H', TICKS_PER_HOUR), ('M', TICKS_PER_MINUTE), ('S', TICKS_PER_SECOND)]),
    ] {
        let mut num = String::new();
        for ch in part.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                num.push(ch);
            } else {
                let unit = units.iter().find(|(u, _)| u.eq_ignore_ascii_case(&ch))?.1;
                total += num.parse::<f64>().ok()? * unit as f64;
                num.clear();
            }
        }
        if !num.is_empty() {
            return None;
        }
    }
    (total.abs() < 9.0e18).then_some(total.round() as i64)
}

/// Reads text written in a .NET-style pattern (`yyyyMMdd`, `dd/MM/yyyy HH:mm`, `d MMM yyyy`).
/// Returns the day and the ticks into the day; parts the pattern lacks default to 1 January of
/// year 1 at midnight, except that a missing year fails.
pub fn parse_with_format(text: &str, pattern: &str, c: &Culture) -> Option<(i64, i64)> {
    let text = text.trim();
    let cs: Vec<char> = pattern.chars().take(256).collect();
    let mut rest = text;
    let (mut y, mut mo, mut d, mut h, mut mi, mut sec) = (None, 1i64, 1i64, 0i64, 0i64, 0f64);
    let mut pm = None;
    // Reads up to `max` digits (exactly `max` when `exact`).
    fn take_digits(rest: &mut &str, max: usize, exact: bool) -> Option<(i64, usize)> {
        let n = rest.bytes().take(max).take_while(u8::is_ascii_digit).count();
        if n == 0 || (exact && n != max) {
            return None;
        }
        let (num, tail) = rest.split_at_checked(n)?;
        *rest = tail;
        Some((num.parse().ok()?, n))
    }
    let mut i = 0;
    while let Some(&ch) = cs.get(i) {
        let mut run = 1;
        while cs.get(i + run) == Some(&ch) {
            run += 1;
        }
        // A numeric field directly followed by another has a fixed width.
        let adjacent = cs.get(i + run).is_some_and(|n| n.is_ascii_alphabetic());
        match ch {
            'y' => {
                let (n, len) = take_digits(&mut rest, if run <= 2 { 2 } else { 4.max(run) }, adjacent || run == 4)?;
                y = Some(if run <= 2 { expand_year(n, len) } else { n });
            }
            'M' if run <= 2 => mo = take_digits(&mut rest, 2, adjacent && run == 2)?.0,
            'M' => {
                let lower = rest.to_lowercase();
                let found = (0..12).find_map(|m| {
                    let full = c.months.get(m)?.to_lowercase();
                    let short = c.month_short(m).to_lowercase();
                    let name = if run >= 4 { full } else { short };
                    lower.starts_with(&name).then_some((m as i64 + 1, name.len()))
                })?;
                mo = found.0;
                rest = rest.get(found.1..)?;
            }
            'd' if run <= 2 => d = take_digits(&mut rest, 2, adjacent && run == 2)?.0,
            'd' => {
                // A day name: skipped.
                let n = rest.find(|x: char| !x.is_alphabetic() && x != '-').unwrap_or(rest.len());
                rest = rest.get(n..)?;
            }
            'H' | 'h' => h = take_digits(&mut rest, 2, adjacent && run == 2)?.0,
            'm' => mi = take_digits(&mut rest, 2, adjacent && run == 2)?.0,
            's' => sec = take_digits(&mut rest, 2, adjacent && run == 2)?.0 as f64,
            'f' | 'F' => {
                let (n, len) = take_digits(&mut rest, 7, false).unwrap_or((0, 1));
                sec += n as f64 / 10f64.powi(len as i32);
            }
            't' => {
                let lower = rest.to_lowercase();
                let (is_pm, len) = if lower.starts_with("pm") {
                    (true, 2)
                } else if lower.starts_with("am") {
                    (false, 2)
                } else if lower.starts_with('p') {
                    (true, 1)
                } else if lower.starts_with('a') {
                    (false, 1)
                } else {
                    return None;
                };
                pm = Some(is_pm);
                rest = rest.get(len..)?;
            }
            '\'' | '"' => {
                let mut j = i + 1;
                while let Some(&q) = cs.get(j) {
                    if q == ch {
                        break;
                    }
                    rest = rest.strip_prefix(q)?;
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            other => {
                for _ in 0..run {
                    rest = rest.strip_prefix(other)?;
                }
            }
        }
        i += run;
    }
    if !rest.is_empty() {
        return None;
    }
    if let Some(is_pm) = pm {
        if !(1..=12).contains(&h) {
            return None;
        }
        h = h % 12 + if is_pm { 12 } else { 0 };
    }
    Some((date(y?, mo, d)?, time(h, mi, sec)?))
}
