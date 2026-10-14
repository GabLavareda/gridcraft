//! `Number.*` and the conversions to the number types (`Int64.From`, `Currency.From`, …).

use super::*;
use crate::culture::Culture;
use crate::error::{EResult, raise, raise_format};
use crate::interp::Interp;
use crate::value::{Builtin, Facet, Value, number_to_text};

pub(crate) const FNS: &[Builtin] = builtin_table![
    "Number.From", 1, 2, from;
    "Number.FromText", 1, 2, from_text;
    "Number.ToText", 1, 3, to_text;
    "Int64.From", 1, 3, |it, a| from_facet(it, a, Facet::Int64);
    "Int32.From", 1, 3, |it, a| from_facet(it, a, Facet::Int32);
    "Int16.From", 1, 3, |it, a| from_facet(it, a, Facet::Int16);
    "Int8.From", 1, 3, |it, a| from_facet(it, a, Facet::Int8);
    "Byte.From", 1, 3, |it, a| from_facet(it, a, Facet::Byte);
    "Double.From", 1, 2, |it, a| from_facet(it, a, Facet::Double);
    "Decimal.From", 1, 2, |it, a| from_facet(it, a, Facet::Decimal);
    "Single.From", 1, 2, |it, a| from_facet(it, a, Facet::Single);
    "Currency.From", 1, 3, |it, a| from_facet(it, a, Facet::Currency);
    "Percentage.From", 1, 2, |it, a| from_facet(it, a, Facet::Percentage);
    "Number.Round", 1, 3, round;
    "Number.RoundUp", 1, 2, |_, a| round_with(a, f64::ceil);
    "Number.RoundDown", 1, 2, |_, a| round_with(a, f64::floor);
    "Number.RoundTowardZero", 1, 2, |_, a| round_with(a, f64::trunc);
    "Number.RoundAwayFromZero", 1, 2, |_, a| round_with(a, |x| x.signum() * x.abs().ceil());
    "Number.Abs", 1, 1, |_, a| math1(a, f64::abs);
    "Number.Sign", 1, 1, |_, a| math1(a, |x| if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 });
    "Number.Sqrt", 1, 1, |_, a| math1(a, f64::sqrt);
    "Number.Exp", 1, 1, |_, a| math1(a, f64::exp);
    "Number.Ln", 1, 1, |_, a| math1(a, f64::ln);
    "Number.Log10", 1, 1, |_, a| math1(a, f64::log10);
    "Number.Log", 1, 2, log;
    "Number.Sin", 1, 1, |_, a| math1(a, f64::sin);
    "Number.Cos", 1, 1, |_, a| math1(a, f64::cos);
    "Number.Tan", 1, 1, |_, a| math1(a, f64::tan);
    "Number.Asin", 1, 1, |_, a| math1(a, f64::asin);
    "Number.Acos", 1, 1, |_, a| math1(a, f64::acos);
    "Number.Atan", 1, 1, |_, a| math1(a, f64::atan);
    "Number.Sinh", 1, 1, |_, a| math1(a, f64::sinh);
    "Number.Cosh", 1, 1, |_, a| math1(a, f64::cosh);
    "Number.Tanh", 1, 1, |_, a| math1(a, f64::tanh);
    "Number.Atan2", 2, 2, |_, a| math2(a, f64::atan2);
    "Number.Power", 2, 2, |_, a| math2(a, f64::powf);
    "Number.Mod", 2, 3, |_, a| math2(a, |x, y| x % y);
    "Number.IntegerDivide", 2, 3, |_, a| math2(a, |x, y| (x / y).trunc());
    "Number.IsNaN", 1, 1, |_, a| Ok(Value::Logical(number(a, 0)?.is_nan()));
    "Number.IsEven", 1, 1, |_, a| Ok(Value::Logical(int(a, 0)? % 2 == 0));
    "Number.IsOdd", 1, 1, |_, a| Ok(Value::Logical(int(a, 0)? % 2 != 0));
    "Number.Factorial", 1, 1, factorial;
    "Number.Combinations", 2, 2, |_, a| choose(a, false);
    "Number.Permutations", 2, 2, |_, a| choose(a, true);
    "Number.BitwiseAnd", 2, 2, |_, a| bits(a, |x, y| x & y);
    "Number.BitwiseOr", 2, 2, |_, a| bits(a, |x, y| x | y);
    "Number.BitwiseXor", 2, 2, |_, a| bits(a, |x, y| x ^ y);
    "Number.BitwiseShiftLeft", 2, 2, |_, a| bits(a, |x, y| x.checked_shl(y as u32).unwrap_or(0));
    "Number.BitwiseShiftRight", 2, 2, |_, a| bits(a, |x, y| x.checked_shr(y as u32).unwrap_or(0));
    "Number.BitwiseNot", 1, 1, |_, a| Ok(match opt_int(a, 0)? { Some(n) => Value::Number(!n as f64), None => Value::Null });
    "Number.Random", 0, 0, |it, _| Ok(Value::Number(it.random()));
    "Number.RandomBetween", 2, 2, |it, a| { let (lo, hi) = (number(a, 0)?, number(a, 1)?); Ok(Value::Number(lo + (hi - lo) * it.random())) };
];

fn from(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let culture = it.culture(opt_text(a, 1)?.as_deref());
    convert::to_number(arg(a, 0), culture)
}

fn from_text(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let culture = it.culture(opt_text(a, 1)?.as_deref());
    match arg(a, 0) {
        Value::Null => Ok(Value::Null),
        v @ Value::Text(s) => match culture.parse_number(s) {
            Some(n) => Ok(Value::Number(n)),
            None => raise_format("The text can't be read as a number.", v.clone()),
        },
        v => mismatch(v, "text"),
    }
}

fn from_facet(it: &mut Interp, a: &[Value], facet: Facet) -> EResult<Value> {
    let culture = it.culture(opt_text(a, 1)?.as_deref());
    match convert::to_number(arg(a, 0), culture)? {
        Value::Number(n) => match (facet, opt_number(a, 2)?) {
            // A rounding mode other than the default, for the whole-number types.
            (Facet::Int64 | Facet::Int32 | Facet::Int16 | Facet::Int8 | Facet::Byte, Some(mode)) => {
                Ok(Value::Number(convert::apply_facet(round_tie(n, mode)?, facet)?))
            }
            _ => Ok(Value::Number(convert::apply_facet(n, facet)?)),
        },
        v => Ok(v),
    }
}

/// Rounds to a whole number, breaking a tie by `mode` (a `RoundingMode` value).
fn round_tie(x: f64, mode: f64) -> EResult<f64> {
    let tie = (x - x.trunc()).abs() == 0.5;
    if !tie {
        return Ok(x.round());
    }
    Ok(match mode as i64 {
        0 => x.ceil(),
        1 => x.floor(),
        2 => x.signum() * x.abs().ceil(),
        3 => x.trunc(),
        4 => convert::round_half_even(x),
        _ => return raise("The rounding mode isn't recognized."),
    })
}

/// `10^digits`, for digits in a sane range.
fn scale_for(digits: Option<i64>) -> EResult<f64> {
    match digits.unwrap_or(0) {
        d @ -300..=300 => Ok(10f64.powi(d as i32)),
        _ => raise("The number of digits is out of range."),
    }
}

/// `x * scale`, snapped to the nearest whole number when it is within rounding error of one:
/// 1.005 × 100 is 100.49999999999999 in binary but a tie on paper.
fn scaled(x: f64, scale: f64) -> f64 {
    let y = x * scale;
    let twice = (y * 2.0).round();
    if ((y * 2.0) - twice).abs() < y.abs() * 4.0 * f64::EPSILON { twice / 2.0 } else { y }
}

fn round(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(x) = opt_number(a, 0)? else { return Ok(Value::Null) };
    let scale = scale_for(opt_int(a, 1)?)?;
    let mode = opt_number(a, 2)?.unwrap_or(4.0);
    if !x.is_finite() {
        return Ok(Value::Number(x));
    }
    Ok(Value::Number(round_tie(scaled(x, scale), mode)? / scale))
}

fn round_with(a: &[Value], f: fn(f64) -> f64) -> EResult<Value> {
    let Some(x) = opt_number(a, 0)? else { return Ok(Value::Null) };
    let scale = scale_for(opt_int(a, 1)?)?;
    if !x.is_finite() {
        return Ok(Value::Number(x));
    }
    let y = x * scale;
    // A value that is whole but for rounding error stays where it is.
    let y = if (y - y.round()).abs() < y.abs() * 4.0 * f64::EPSILON { y.round() } else { y };
    Ok(Value::Number(f(y) / scale))
}

fn math1(a: &[Value], f: fn(f64) -> f64) -> EResult<Value> {
    Ok(match opt_number(a, 0)? {
        Some(x) => Value::Number(f(x)),
        None => Value::Null,
    })
}

fn math2(a: &[Value], f: fn(f64, f64) -> f64) -> EResult<Value> {
    Ok(match (opt_number(a, 0)?, opt_number(a, 1)?) {
        (Some(x), Some(y)) => Value::Number(f(x, y)),
        _ => Value::Null,
    })
}

fn log(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(x) = opt_number(a, 0)? else { return Ok(Value::Null) };
    Ok(Value::Number(match opt_number(a, 1)? {
        Some(base) => x.ln() / base.ln(),
        None => x.ln(),
    }))
}

fn bits(a: &[Value], f: fn(i64, i64) -> i64) -> EResult<Value> {
    Ok(match (opt_int(a, 0)?, opt_int(a, 1)?) {
        (Some(x), Some(y)) => Value::Number(f(x, y) as f64),
        _ => Value::Null,
    })
}

fn factorial(_: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(n) = opt_int(a, 0)? else { return Ok(Value::Null) };
    if n < 0 {
        return raise("A factorial needs a number that isn't negative.");
    }
    Ok(Value::Number((1..=n.min(171)).fold(1.0, |acc, k| acc * k as f64)))
}

fn choose(a: &[Value], ordered: bool) -> EResult<Value> {
    let (Some(n), Some(k)) = (opt_int(a, 0)?, opt_int(a, 1)?) else { return Ok(Value::Null) };
    if n < 0 || k < 0 || k > n {
        return raise("The arguments must satisfy 0 ≤ k ≤ n.");
    }
    let k = if ordered { k } else { k.min(n - k) };
    let mut r = 1.0f64;
    for i in 0..k.min(100_000) {
        r *= (n - i) as f64;
        if !ordered {
            r /= (i + 1) as f64;
        }
    }
    Ok(Value::Number(r.round()))
}

// ---------------------------------------------------------------- Number.ToText

fn to_text(it: &mut Interp, a: &[Value]) -> EResult<Value> {
    let Some(n) = opt_number(a, 0)? else { return Ok(Value::Null) };
    let culture = it.culture(opt_text(a, 2)?.as_deref());
    match opt_text(a, 1)? {
        None => Ok(Value::text(culture.number_to_text(n))),
        Some(f) => Ok(Value::text(format_number(n, &f, culture)?)),
    }
}

/// Formats with a standard format (`F2`, `N0`, `P1`, `E3`, `D5`, `X`, `G`) or a custom pattern
/// (`0.00`, `#,##0.0`, `0%`).
pub(crate) fn format_number(n: f64, format: &str, c: &Culture) -> EResult<String> {
    if !n.is_finite() {
        return Ok(c.number_to_text(n));
    }
    let mut chars = format.chars();
    let (Some(spec), digits) = (chars.next(), chars.as_str()) else { return Ok(c.number_to_text(n)) };
    let standard = spec.is_ascii_alphabetic() && digits.len() <= 3 && digits.bytes().all(|b| b.is_ascii_digit());
    if !standard {
        return Ok(format_custom(n, format, c));
    }
    let precision: Option<usize> = digits.parse().ok();
    Ok(match spec.to_ascii_uppercase() {
        'F' => c.fixed(n, precision.unwrap_or(2), false),
        'N' => c.fixed(n, precision.unwrap_or(2), true),
        'P' => format!("{}%", c.fixed(n * 100.0, precision.unwrap_or(2), true)),
        'C' => {
            let body = c.fixed(n.abs(), precision.unwrap_or(2), true);
            let symbol = match c.name {
                "pt-BR" => "R$ ",
                "en-GB" => "£",
                "ja-JP" | "zh-CN" => "¥",
                name if name.starts_with("en") || name.is_empty() => "$",
                _ => "",
            };
            let suffix = if symbol.is_empty() { " €" } else { "" };
            format!("{}{symbol}{body}{suffix}", if n < 0.0 { "-" } else { "" })
        }
        'E' => {
            let s = format!("{:.*e}", precision.unwrap_or(6).min(99), n);
            let (mantissa, exp) = s.split_once('e').unwrap_or((s.as_str(), "0"));
            let exp: i32 = exp.parse().unwrap_or(0);
            let e = if spec.is_ascii_lowercase() { 'e' } else { 'E' };
            format!("{}{e}{}{:03}", mantissa.replace('.', &c.decimal.to_string()), if exp < 0 { '-' } else { '+' }, exp.abs())
        }
        'D' => {
            if n.fract() != 0.0 {
                return raise("The D format needs a whole number.");
            }
            let s = format!("{:0width$}", n.abs() as u64, width = precision.unwrap_or(1).min(99));
            if n < 0.0 { format!("-{s}") } else { s }
        }
        'X' => {
            if n.fract() != 0.0 {
                return raise("The X format needs a whole number.");
            }
            let width = precision.unwrap_or(1).min(99);
            if spec.is_ascii_lowercase() { format!("{:0width$x}", n as i64) } else { format!("{:0width$X}", n as i64) }
        }
        'G' | 'R' => match precision {
            Some(p) if p > 0 && spec.eq_ignore_ascii_case(&'G') => {
                // `p` significant digits.
                let s = format!("{:.*e}", p.saturating_sub(1).min(99), n);
                let rounded: f64 = s.parse().unwrap_or(n);
                number_to_text(rounded).replace('.', &c.decimal.to_string())
            }
            _ => c.number_to_text(n),
        },
        _ => format_custom(n, format, c),
    })
}

/// A custom pattern: `0` shows a digit always, `#` only when it matters, `.` is the decimal
/// point, `,` between digits groups thousands, `%` multiplies by 100; other characters are kept.
fn format_custom(n: f64, pattern: &str, c: &Culture) -> String {
    // Only the first section (positive numbers) of `positive;negative;zero` is used.
    let pattern = pattern.split(';').next().unwrap_or(pattern);
    let percent = pattern.contains('%');
    let (int_part, frac_part) = pattern.split_once('.').unwrap_or((pattern, ""));
    let min_decimals = frac_part.chars().filter(|ch| *ch == '0').count();
    let max_decimals = frac_part.chars().filter(|ch| matches!(ch, '0' | '#')).count();
    let min_int = int_part.chars().filter(|ch| *ch == '0').count();
    let first_digit = int_part.find(['0', '#']);
    let last_digit = int_part.rfind(['0', '#']);
    let grouped = match (first_digit, last_digit) {
        (Some(x), Some(y)) => int_part.get(x..y).is_some_and(|s| s.contains(',')),
        _ => false,
    };
    let value = if percent { n * 100.0 } else { n };
    let mut body = c.fixed(value.abs(), max_decimals, grouped);
    // Drop optional trailing decimals.
    if max_decimals > min_decimals
        && let Some(dot) = body.rfind(c.decimal)
    {
        let keep = dot + c.decimal.len_utf8() + min_decimals;
        while body.len() > keep && body.ends_with('0') {
            body.pop();
        }
        if body.ends_with(c.decimal) {
            body.pop();
        }
    }
    // Pad or trim the integer part to the pattern's minimum digits.
    let int_len = body.find(c.decimal).unwrap_or(body.len());
    let int_digits = body.get(..int_len).map_or(0, |s| s.chars().filter(char::is_ascii_digit).count());
    if min_int == 0 && body.get(..int_len) == Some("0") && body.len() > int_len {
        body.remove(0);
    } else if int_digits < min_int {
        body.insert_str(0, &"0".repeat(min_int - int_digits));
    }
    let mut out = String::new();
    let is_literal = |ch: char| !matches!(ch, '0' | '#' | ',' | '.');
    let prefix: String = match first_digit {
        Some(x) => int_part.get(..x).unwrap_or("").chars().filter(|ch| is_literal(*ch)).collect(),
        None => int_part.chars().filter(|ch| is_literal(*ch)).collect(),
    };
    let tail = if pattern.contains('.') { frac_part } else { last_digit.and_then(|y| int_part.get(y + 1..)).unwrap_or("") };
    let suffix: String = tail.chars().filter(|ch| is_literal(*ch)).collect();
    if n < 0.0 && body.chars().any(|ch| ch.is_ascii_digit() && ch != '0') {
        out.push('-');
    }
    out.push_str(&prefix);
    out.push_str(&body);
    out.push_str(&suffix);
    out
}
