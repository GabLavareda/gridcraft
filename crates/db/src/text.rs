//! Dates, times and durations written as text (how PostgreSQL, MySQL and SQLite send them),
//! read into M's representation. Anything that doesn't parse gives `None`, and the caller keeps
//! the text.

use crate::{TICKS_PER_DAY, TICKS_PER_SECOND};

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar.
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

fn digits(s: &str) -> Option<i64> {
    if s.is_empty() || s.len() > 9 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `YYYY-MM-DD` to days since 1970-01-01. Years 1 to 9999.
pub fn parse_date(s: &str) -> Option<i32> {
    let mut parts = s.trim().splitn(3, '-');
    let (y, m, d) = (digits(parts.next()?)?, digits(parts.next()?)?, digits(parts.next()?)?);
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) {
        return None;
    }
    let (m, d) = (m as u32, d as u32);
    if d == 0 || d > days_in_month(y, m) {
        return None;
    }
    i32::try_from(days_from_civil(y, m, d)).ok()
}

/// A fraction of a second (the digits after the point) in ticks; digits past the seventh are
/// dropped.
fn fraction(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut ticks = 0i64;
    for (i, b) in s.bytes().take(7).enumerate() {
        ticks += i64::from(b - b'0') * 10i64.pow(6 - i as u32);
    }
    Some(ticks)
}

/// `HH:MM[:SS[.fffffff]]` to ticks since midnight, below 24 hours.
pub fn parse_time(s: &str) -> Option<i64> {
    let t = parse_clock(s.trim(), 23)?;
    (t < TICKS_PER_DAY).then_some(t)
}

/// Hours, minutes, seconds and fraction in ticks; hours up to `max_hours`.
fn parse_clock(s: &str, max_hours: i64) -> Option<i64> {
    let (main, frac) = match s.split_once('.') {
        Some((a, b)) => (a, fraction(b)?),
        None => (s, 0),
    };
    let mut parts = main.split(':');
    let h = digits(parts.next()?)?;
    let m = digits(parts.next()?)?;
    let sec = match parts.next() {
        Some(x) => digits(x)?,
        None => 0,
    };
    if parts.next().is_some() || h > max_hours || m > 59 || sec > 60 {
        return None;
    }
    Some(((h * 60 + m) * 60 + sec) * TICKS_PER_SECOND + frac)
}

/// `YYYY-MM-DD[ T]HH:MM[:SS[.fffffff]]` with an optional offset (`Z`, `+HH`, `+HH:MM`, `+HHMM`):
/// local ticks since 1970-01-01T00:00 and the offset in minutes when one was written. A date
/// alone is midnight.
pub fn parse_datetime(s: &str) -> Option<(i64, Option<i32>)> {
    let s = s.trim();
    let (date, rest) = match s.find(['T', 't', ' ']) {
        Some(i) => (s.get(..i)?, s.get(i + 1..)?.trim_start()),
        None => (s, ""),
    };
    let days = i64::from(parse_date(date)?);
    if rest.is_empty() {
        return Some((days * TICKS_PER_DAY, None));
    }
    // The offset starts at Z or at a sign after the time.
    let (clock, offset) = match rest.find(['Z', 'z', '+', '-']) {
        Some(i) => {
            let (c, o) = rest.split_at(i);
            (c.trim_end(), Some(parse_offset(o)?))
        }
        None => (rest, None),
    };
    let t = parse_clock(clock, 23)?;
    Some((days * TICKS_PER_DAY + t, offset))
}

/// `Z`, `+HH`, `+HH:MM`, `+HHMM` (or `-`) to minutes.
fn parse_offset(s: &str) -> Option<i32> {
    if s.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, s.get(1..)?),
        b'-' => (-1, s.get(1..)?),
        _ => return None,
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (digits(h)?, digits(m)?),
        None if rest.len() == 4 => (digits(rest.get(..2)?)?, digits(rest.get(2..)?)?),
        None => (digits(rest)?, 0),
    };
    if h > 14 || m > 59 {
        return None;
    }
    Some(sign * (h * 60 + m) as i32)
}

/// A duration: MySQL's `[-]HHH:MM:SS[.ffffff]`, or ISO 8601 `P…` as PostgreSQL writes intervals
/// with `IntervalStyle = iso_8601` (a month counts 30 days, a year 365.25).
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, body) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let ticks = if body.starts_with('P') || body.starts_with('p') { parse_iso_duration(body)? } else { parse_clock(body, 99_999_999)? };
    Some(if neg { -ticks } else { ticks })
}

fn parse_iso_duration(s: &str) -> Option<i64> {
    let body = s.get(1..)?;
    let mut ticks: i64 = 0;
    let mut in_time = false;
    let mut num = String::new();
    for ch in body.chars() {
        match ch {
            'T' | 't' => in_time = true,
            '0'..='9' | '.' | '-' | '+' | ',' => num.push(if ch == ',' { '.' } else { ch }),
            unit => {
                let n: f64 = num.parse().ok()?;
                num.clear();
                let per = match (unit.to_ascii_uppercase(), in_time) {
                    ('Y', false) => 365.25 * TICKS_PER_DAY as f64,
                    ('M', false) => 30.0 * TICKS_PER_DAY as f64,
                    ('W', false) => 7.0 * TICKS_PER_DAY as f64,
                    ('D', false) => TICKS_PER_DAY as f64,
                    ('H', true) => 3600.0 * TICKS_PER_SECOND as f64,
                    ('M', true) => 60.0 * TICKS_PER_SECOND as f64,
                    ('S', true) => TICKS_PER_SECOND as f64,
                    _ => return None,
                };
                let add = (n * per).round();
                if !add.is_finite() || add.abs() > 9.0e18 {
                    return None;
                }
                ticks = ticks.checked_add(add as i64)?;
            }
        }
    }
    num.is_empty().then_some(ticks)
}
