//! `Date.*`, `Time.*`, `DateTime.*`, `DateTimeZone.*`, `Duration.*` and the `#date`-style
//! constructors.

use std::rc::Rc;

use super::*;
use crate::culture::Culture;
use crate::datetime::{self, Moment, TICKS_PER_DAY, TICKS_PER_HOUR, TICKS_PER_MINUTE, TICKS_PER_SECOND};
use crate::error::{EResult, raise, raise_format};
use crate::interp::Interp;
use crate::value::{Builtin, Value};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "#date", 3, 3, new_date;
    "#time", 3, 3, new_time;
    "#datetime", 6, 6, new_datetime;
    "#datetimezone", 8, 8, new_datetimezone;
    "#duration", 4, 4, new_duration;
    "Date.From", 1, 2, |it, a| convert::to_date(arg(a, 0), it.culture(opt_text(a, 1)?.as_deref()));
    "DateTime.From", 1, 2, |it, a| convert::to_datetime(arg(a, 0), it.culture(opt_text(a, 1)?.as_deref()));
    "DateTimeZone.From", 1, 2, |it, a| convert::to_datetimezone(arg(a, 0), it.culture(opt_text(a, 1)?.as_deref()), it.local_offset());
    "Time.From", 1, 2, |it, a| convert::to_time(arg(a, 0), it.culture(opt_text(a, 1)?.as_deref()));
    "Duration.From", 1, 1, |_, a| convert::to_duration(arg(a, 0));
    "Date.FromText", 1, 2, |it, a| from_text(it, a, Kind::Date);
    "DateTime.FromText", 1, 2, |it, a| from_text(it, a, Kind::DateTime);
    "DateTimeZone.FromText", 1, 2, |it, a| from_text(it, a, Kind::Zone(0));
    "Time.FromText", 1, 2, |it, a| from_text(it, a, Kind::Time);
    "Duration.FromText", 1, 1, |_, a| match arg(a, 0) { Value::Null => Ok(Value::Null), v @ Value::Text(_) => convert::to_duration(v), v => mismatch(v, "text") };
    "Date.ToText", 1, 3, to_text;
    "DateTime.ToText", 1, 3, to_text;
    "DateTimeZone.ToText", 1, 3, to_text;
    "Time.ToText", 1, 3, to_text;
    "Duration.ToText", 1, 2, |_, a| Ok(match arg(a, 0) { Value::Null => Value::Null, Value::Duration(t) => Value::text(datetime::duration_to_text(*t)), v => return mismatch(v, "a duration") });
    "Date.Year", 1, 1, |_, a| part(a, |m| m.ymd().0);
    "Date.Month", 1, 1, |_, a| part(a, |m| m.ymd().1);
    "Date.Day", 1, 1, |_, a| part(a, |m| m.ymd().2);
    "Date.QuarterOfYear", 1, 1, |_, a| part(a, |m| (m.ymd().1 - 1) / 3 + 1);
    "Date.DayOfYear", 1, 1, |_, a| part(a, |m| datetime::day_of_year(m.days));
    "Date.DaysInMonth", 1, 1, |_, a| part(a, |m| datetime::days_in_month(m.ymd().0, m.ymd().1));
    "Date.IsLeapYear", 1, 1, |_, a| Ok(match moment(arg(a, 0))? { Some(m) => Value::Logical(datetime::is_leap(m.ymd().0)), None => Value::Null });
    "Date.DayOfWeek", 1, 2, day_of_week;
    "Date.WeekOfYear", 1, 2, |it, a| week_of(it, a, false);
    "Date.WeekOfMonth", 1, 2, |it, a| week_of(it, a, true);
    "Date.MonthName", 1, 2, |it, a| name_of(it, a, true);
    "Date.DayOfWeekName", 1, 2, |it, a| name_of(it, a, false);
    "Date.AddDays", 2, 2, |_, a| add(a, Unit::Days);
    "Date.AddWeeks", 2, 2, |_, a| add(a, Unit::Weeks);
    "Date.AddMonths", 2, 2, |_, a| add(a, Unit::Months);
    "Date.AddQuarters", 2, 2, |_, a| add(a, Unit::Quarters);
    "Date.AddYears", 2, 2, |_, a| add(a, Unit::Years);
    "Date.StartOfDay", 1, 1, |it, a| edge(it, a, Span::Day, false);
    "Date.EndOfDay", 1, 1, |it, a| edge(it, a, Span::Day, true);
    "Date.StartOfWeek", 1, 2, |it, a| edge(it, a, Span::Week, false);
    "Date.EndOfWeek", 1, 2, |it, a| edge(it, a, Span::Week, true);
    "Date.StartOfMonth", 1, 1, |it, a| edge(it, a, Span::Month, false);
    "Date.EndOfMonth", 1, 1, |it, a| edge(it, a, Span::Month, true);
    "Date.StartOfQuarter", 1, 1, |it, a| edge(it, a, Span::Quarter, false);
    "Date.EndOfQuarter", 1, 1, |it, a| edge(it, a, Span::Quarter, true);
    "Date.StartOfYear", 1, 1, |it, a| edge(it, a, Span::Year, false);
    "Date.EndOfYear", 1, 1, |it, a| edge(it, a, Span::Year, true);
    "Date.IsInCurrentDay", 1, 1, |it, a| is_in(it, a, Span::Day, 0);
    "Date.IsInPreviousDay", 1, 1, |it, a| is_in(it, a, Span::Day, -1);
    "Date.IsInNextDay", 1, 1, |it, a| is_in(it, a, Span::Day, 1);
    "Date.IsInCurrentWeek", 1, 1, |it, a| is_in(it, a, Span::Week, 0);
    "Date.IsInPreviousWeek", 1, 1, |it, a| is_in(it, a, Span::Week, -1);
    "Date.IsInNextWeek", 1, 1, |it, a| is_in(it, a, Span::Week, 1);
    "Date.IsInCurrentMonth", 1, 1, |it, a| is_in(it, a, Span::Month, 0);
    "Date.IsInPreviousMonth", 1, 1, |it, a| is_in(it, a, Span::Month, -1);
    "Date.IsInNextMonth", 1, 1, |it, a| is_in(it, a, Span::Month, 1);
    "Date.IsInCurrentQuarter", 1, 1, |it, a| is_in(it, a, Span::Quarter, 0);
    "Date.IsInPreviousQuarter", 1, 1, |it, a| is_in(it, a, Span::Quarter, -1);
    "Date.IsInNextQuarter", 1, 1, |it, a| is_in(it, a, Span::Quarter, 1);
    "Date.IsInCurrentYear", 1, 1, |it, a| is_in(it, a, Span::Year, 0);
    "Date.IsInPreviousYear", 1, 1, |it, a| is_in(it, a, Span::Year, -1);
    "Date.IsInNextYear", 1, 1, |it, a| is_in(it, a, Span::Year, 1);
    "Date.IsInYearToDate", 1, 1, is_in_year_to_date;
    "DateTime.Date", 1, 1, |_, a| Ok(match moment(arg(a, 0))? { Some(m) => Value::Date(m.days as i32), None => Value::Null });
    "DateTime.Time", 1, 1, |_, a| Ok(match moment(arg(a, 0))? { Some(m) => Value::Time(m.tod), None => Value::Null });
    "DateTime.LocalNow", 0, 0, |it, _| Ok(Value::DateTime(it.now()?.local_ticks));
    "DateTime.FixedLocalNow", 0, 0, |it, _| Ok(Value::DateTime(it.now()?.local_ticks));
    "DateTimeZone.LocalNow", 0, 0, |it, _| { let n = it.now()?; Ok(Value::DateTimeZone(n.local_ticks, n.offset_minutes)) };
    "DateTimeZone.FixedLocalNow", 0, 0, |it, _| { let n = it.now()?; Ok(Value::DateTimeZone(n.local_ticks, n.offset_minutes)) };
    "DateTimeZone.UtcNow", 0, 0, |it, _| { let n = it.now()?; Ok(Value::DateTimeZone(datetime::utc_ticks(n.local_ticks, n.offset_minutes), 0)) };
    "DateTimeZone.FixedUtcNow", 0, 0, |it, _| { let n = it.now()?; Ok(Value::DateTimeZone(datetime::utc_ticks(n.local_ticks, n.offset_minutes), 0)) };
    "DateTimeZone.ZoneHours", 1, 1, |_, a| Ok(match arg(a, 0) { Value::Null => Value::Null, Value::DateTimeZone(_, off) => Value::Number((off / 60) as f64), v => return mismatch(v, "a date, time and time zone") });
    "DateTimeZone.ZoneMinutes", 1, 1, |_, a| Ok(match arg(a, 0) { Value::Null => Value::Null, Value::DateTimeZone(_, off) => Value::Number((off % 60) as f64), v => return mismatch(v, "a date, time and time zone") });
    "DateTimeZone.RemoveZone", 1, 1, |_, a| Ok(match arg(a, 0) { Value::Null => Value::Null, Value::DateTimeZone(t, _) => Value::DateTime(*t), v => return mismatch(v, "a date, time and time zone") });
    "DateTimeZone.SwitchZone", 2, 3, switch_zone;
    "DateTimeZone.ToUtc", 1, 1, |_, a| to_zone(a, 0);
    "DateTimeZone.ToLocal", 1, 1, |it, a| { let off = it.local_offset(); to_zone(a, off) };
    "DateTime.AddZone", 2, 3, add_zone;
    "Time.Hour", 1, 1, |_, a| part(a, |m| m.tod / TICKS_PER_HOUR);
    "Time.Minute", 1, 1, |_, a| part(a, |m| (m.tod % TICKS_PER_HOUR) / TICKS_PER_MINUTE);
    "Time.Second", 1, 1, |_, a| Ok(match moment(arg(a, 0))? { Some(m) => Value::Number((m.tod % TICKS_PER_MINUTE) as f64 / TICKS_PER_SECOND as f64), None => Value::Null });
    "Time.StartOfHour", 1, 1, |_, a| hour_edge(a, false);
    "Time.EndOfHour", 1, 1, |_, a| hour_edge(a, true);
    "Duration.Days", 1, 1, |_, a| duration_part(a, |t| (t / TICKS_PER_DAY) as f64);
    "Duration.Hours", 1, 1, |_, a| duration_part(a, |t| ((t % TICKS_PER_DAY) / TICKS_PER_HOUR) as f64);
    "Duration.Minutes", 1, 1, |_, a| duration_part(a, |t| ((t % TICKS_PER_HOUR) / TICKS_PER_MINUTE) as f64);
    "Duration.Seconds", 1, 1, |_, a| duration_part(a, |t| (t % TICKS_PER_MINUTE) as f64 / TICKS_PER_SECOND as f64);
    "Duration.TotalDays", 1, 1, |_, a| duration_part(a, |t| t as f64 / TICKS_PER_DAY as f64);
    "Duration.TotalHours", 1, 1, |_, a| duration_part(a, |t| t as f64 / TICKS_PER_HOUR as f64);
    "Duration.TotalMinutes", 1, 1, |_, a| duration_part(a, |t| t as f64 / TICKS_PER_MINUTE as f64);
    "Duration.TotalSeconds", 1, 1, |_, a| duration_part(a, |t| t as f64 / TICKS_PER_SECOND as f64);
];

/// Which kind of value a moment came from, and goes back to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Date,
    Time,
    DateTime,
    /// With the offset from UTC in minutes.
    Zone(i32),
}

/// A date, time, date-time or zoned date-time, taken apart.
#[derive(Clone, Copy)]
struct Parts {
    days: i64,
    tod: i64,
    kind: Kind,
}

impl Parts {
    fn ymd(&self) -> (i64, i64, i64) {
        datetime::civil_from_days(self.days)
    }
    /// Back to a value of the kind it came from.
    fn value(self) -> EResult<Value> {
        if !datetime::valid_day(self.days) {
            return raise("The date is outside the years 1 to 9999.");
        }
        Ok(match self.kind {
            Kind::Date => Value::Date(self.days as i32),
            Kind::Time => Value::Time(self.tod),
            Kind::DateTime => Value::DateTime(datetime::join_ticks(self.days, self.tod)),
            Kind::Zone(off) => Value::DateTimeZone(datetime::join_ticks(self.days, self.tod), off),
        })
    }
}

/// The parts of a date-like value; `None` for null.
fn moment(v: &Value) -> EResult<Option<Parts>> {
    Ok(Some(match v {
        Value::Null => return Ok(None),
        Value::Date(d) => Parts { days: *d as i64, tod: 0, kind: Kind::Date },
        Value::Time(t) => Parts { days: 0, tod: *t, kind: Kind::Time },
        Value::DateTime(t) => {
            let (days, tod) = datetime::split_ticks(*t);
            Parts { days, tod, kind: Kind::DateTime }
        }
        Value::DateTimeZone(t, off) => {
            let (days, tod) = datetime::split_ticks(*t);
            Parts { days, tod, kind: Kind::Zone(*off) }
        }
        v => return mismatch(v, "a date or a time"),
    }))
}

fn part(a: &[Value], f: fn(&Parts) -> i64) -> EResult<Value> {
    Ok(match moment(arg(a, 0))? {
        Some(m) => Value::Number(f(&m) as f64),
        None => Value::Null,
    })
}

fn duration_part(a: &[Value], f: fn(i64) -> f64) -> EResult<Value> {
    Ok(match arg(a, 0) {
        Value::Null => Value::Null,
        Value::Duration(t) => Value::Number(f(*t)),
        v => return mismatch(v, "a duration"),
    })
}

// ---------------------------------------------------------------- constructors

fn new_date(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    match datetime::date(int(a, 0)?, int(a, 1)?, int(a, 2)?) {
        Some(d) => Ok(Value::Date(d as i32)),
        None => raise("This isn't a date: the year is 1 to 9999, the month 1 to 12, and the day must exist in that month."),
    }
}

fn time_of(a: &[Value], i: usize) -> EResult<i64> {
    match datetime::time(int(a, i)?, int(a, i + 1)?, number(a, i + 2)?) {
        Some(t) => Ok(t),
        None => raise("This isn't a time: hours are 0 to 23, minutes 0 to 59 and seconds 0 up to 60."),
    }
}

fn new_time(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    Ok(Value::Time(time_of(a, 0)? % TICKS_PER_DAY))
}

fn datetime_of(it: &mut Interp, a: &[Value]) -> EResult<i64> {
    let Value::Date(d) = new_date(it, a)? else { return raise("This isn't a date.") };
    Ok(datetime::join_ticks(d as i64, time_of(a, 3)?))
}

fn new_datetime(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    Ok(Value::DateTime(datetime_of(it, a)?))
}

fn zone_of(a: &[Value], i: usize) -> EResult<i32> {
    let (h, m) = (int(a, i)?, opt_int(a, i + 1)?.unwrap_or(0));
    if !(-14..=14).contains(&h) || !(-59..=59).contains(&m) {
        return raise("The time zone offset is at most 14 hours.");
    }
    Ok((h * 60 + m) as i32)
}

fn new_datetimezone(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    Ok(Value::DateTimeZone(datetime_of(it, a)?, zone_of(a, 6)?))
}

fn new_duration(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let (d, h, m, s) = (int(a, 0)?, int(a, 1)?, int(a, 2)?, number(a, 3)?);
    let total = d as f64 * TICKS_PER_DAY as f64 + h as f64 * TICKS_PER_HOUR as f64 + m as f64 * TICKS_PER_MINUTE as f64 + s * TICKS_PER_SECOND as f64;
    if !total.is_finite() || total.abs() > 9.0e18 {
        return raise("The duration is too long.");
    }
    Ok(Value::Duration(total.round() as i64))
}

// ---------------------------------------------------------------- text

/// The format and culture of an options argument: a record `[Format, Culture]`, or text that is
/// a format (`as_format`) or a culture name.
fn text_options(it: &Interp, a: &[Value], i: usize, as_format: bool) -> EResult<(Option<Rc<str>>, &'static Culture)> {
    let (mut format, mut culture) = (None, opt_text(a, i + 1)?);
    match arg(a, i) {
        Value::Null => {}
        Value::Text(s) if as_format => format = Some(s.clone()),
        Value::Text(s) => culture = Some(s.clone()),
        Value::Record(r) => {
            if let Some(Value::Text(f)) = r.get("Format")? {
                format = Some(f);
            }
            if let Some(Value::Text(c)) = r.get("Culture")? {
                culture = Some(c);
            }
        }
        v => return mismatch(v, "a format or options"),
    }
    Ok((format, it.culture(culture.as_deref())))
}

fn from_text(it: &mut Interp, a: &[Value], kind: Kind) -> EResult<Value> {
    let v = arg(a, 0);
    let s = match v {
        Value::Null => return Ok(Value::Null),
        Value::Text(s) => s,
        v => return mismatch(v, "text"),
    };
    let (format, culture) = text_options(it, a, 1, false)?;
    if let Some(f) = format {
        let Some((days, tod)) = datetime::parse_with_format(s, &f, culture) else {
            return raise_format("The text doesn't match the format.", v.clone());
        };
        return Parts { days, tod, kind: if let Kind::Zone(_) = kind { Kind::Zone(it.local_offset()) } else { kind } }.value();
    }
    match kind {
        Kind::Date => convert::to_date(v, culture),
        Kind::Time => convert::to_time(v, culture),
        Kind::DateTime => convert::to_datetime(v, culture),
        Kind::Zone(_) => convert::to_datetimezone(v, culture, it.local_offset()),
    }
}

fn to_text(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let (format, culture) = text_options(it, a, 1, true)?;
    match format {
        Some(f) => {
            let offset = match m.kind {
                Kind::Zone(off) => Some(off),
                _ => None,
            };
            Ok(Value::text(datetime::format(Moment { days: m.days, tod: m.tod, offset }, &f, culture)))
        }
        None => convert::to_text(arg(a, 0), culture),
    }
}

// ---------------------------------------------------------------- calendar

/// The first day of the week from an optional `Day.…` argument, else from the culture.
fn first_day(it: &Interp, a: &[Value], i: usize) -> EResult<i64> {
    match opt_int(a, i)? {
        Some(d) if (0..7).contains(&d) => Ok(d),
        Some(_) => raise("The first day of the week is a Day value, 0 to 6."),
        None => Ok(it.culture(None).first_day_of_week()),
    }
}

fn day_of_week(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let first = first_day(it, a, 1)?;
    Ok(Value::Number((datetime::weekday(m.days) - first).rem_euclid(7) as f64))
}

fn week_of(it: &mut Interp, a: &[Value], of_month: bool) -> EResult<Value> {
    let Some(m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let first = first_day(it, a, 1)?;
    let (y, mo, _) = m.ymd();
    let start = datetime::days_from_civil(y, if of_month { mo } else { 1 }, 1);
    let lead = (datetime::weekday(start) - first).rem_euclid(7);
    Ok(Value::Number(((m.days - start + lead) / 7 + 1) as f64))
}

fn name_of(it: &mut Interp, a: &[Value], month: bool) -> EResult<Value> {
    let Some(m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let culture = it.culture(opt_text(a, 1)?.as_deref());
    let name = if month { culture.months.get((m.ymd().1 - 1) as usize) } else { culture.days.get(datetime::weekday(m.days) as usize) };
    Ok(Value::text(name.copied().unwrap_or("")))
}

enum Unit {
    Days,
    Weeks,
    Months,
    Quarters,
    Years,
}

fn add(a: &[Value], unit: Unit) -> EResult<Value> {
    let Some(mut m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let n = int(a, 1)?;
    let months = |k: i64| n.checked_mul(k).and_then(|total| datetime::add_months(m.days, total));
    let days = match unit {
        Unit::Days => m.days.checked_add(n),
        Unit::Weeks => n.checked_mul(7).and_then(|d| m.days.checked_add(d)),
        Unit::Months => months(1),
        Unit::Quarters => months(3),
        Unit::Years => months(12),
    };
    match days {
        Some(d) => {
            m.days = d;
            m.value()
        }
        None => raise("The date is outside the years 1 to 9999."),
    }
}

#[derive(Clone, Copy)]
enum Span {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

/// The first and last day of the span that holds `days`.
fn span_of(days: i64, span: Span, first_day: i64) -> (i64, i64) {
    let (y, m, _) = datetime::civil_from_days(days);
    match span {
        Span::Day => (days, days),
        Span::Week => {
            let start = days - (datetime::weekday(days) - first_day).rem_euclid(7);
            (start, start + 6)
        }
        Span::Month => (datetime::days_from_civil(y, m, 1), datetime::days_from_civil(y, m, datetime::days_in_month(y, m))),
        Span::Quarter => {
            let q = (m - 1) / 3 * 3 + 1;
            (datetime::days_from_civil(y, q, 1), datetime::days_from_civil(y, q + 2, datetime::days_in_month(y, q + 2)))
        }
        Span::Year => (datetime::days_from_civil(y, 1, 1), datetime::days_from_civil(y, 12, 31)),
    }
}

fn edge(it: &mut Interp, a: &[Value], span: Span, end: bool) -> EResult<Value> {
    let Some(mut m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let first = match span {
        Span::Week => first_day(it, a, 1)?,
        _ => 0,
    };
    let (start, last) = span_of(m.days, span, first);
    m.days = if end { last } else { start };
    // The last tick of the day, for values that carry a time.
    m.tod = if end { TICKS_PER_DAY - 1 } else { 0 };
    m.value()
}

/// Whether the date falls in the current span (`shift` 0), the previous (-1) or the next (1).
fn is_in(it: &mut Interp, a: &[Value], span: Span, shift: i64) -> EResult<Value> {
    let Some(m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let today = datetime::split_ticks(it.now()?.local_ticks).0;
    let first = it.culture(None).first_day_of_week();
    let (start, last) = span_of(today, span, first);
    // Step to the neighbouring span through the day just outside this one.
    let (start, last) = match shift {
        0 => (start, last),
        s if s < 0 => span_of(start - 1, span, first),
        _ => span_of(last + 1, span, first),
    };
    Ok(Value::Logical((start..=last).contains(&m.days)))
}

fn is_in_year_to_date(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let today = datetime::split_ticks(it.now()?.local_ticks).0;
    let (start, _) = span_of(today, Span::Year, 0);
    Ok(Value::Logical((start..=today).contains(&m.days)))
}

fn hour_edge(a: &[Value], end: bool) -> EResult<Value> {
    let Some(mut m) = moment(arg(a, 0))? else { return Ok(Value::Null) };
    let start = m.tod - m.tod % TICKS_PER_HOUR;
    m.tod = if end { start + TICKS_PER_HOUR - 1 } else { start };
    m.value()
}

// ---------------------------------------------------------------- zones

fn switch_zone(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    match arg(a, 0) {
        Value::Null => Ok(Value::Null),
        Value::DateTimeZone(t, off) => {
            let new = zone_of(a, 1)?;
            Ok(Value::DateTimeZone(t + (new - off) as i64 * TICKS_PER_MINUTE, new))
        }
        v => mismatch(v, "a date, time and time zone"),
    }
}

fn to_zone(a: &[Value], offset: i32) -> EResult<Value> {
    match arg(a, 0) {
        Value::Null => Ok(Value::Null),
        Value::DateTimeZone(t, off) => Ok(Value::DateTimeZone(t + (offset - off) as i64 * TICKS_PER_MINUTE, offset)),
        v => mismatch(v, "a date, time and time zone"),
    }
}

fn add_zone(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    match arg(a, 0) {
        Value::Null => Ok(Value::Null),
        Value::DateTime(t) => Ok(Value::DateTimeZone(*t, zone_of(a, 1)?)),
        v => mismatch(v, "a date and time"),
    }
}
