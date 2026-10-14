//! Cultures: how numbers and dates are written and read (`"en-US"`, `"pt-BR"`, …). A small
//! built-in table; unknown cultures fall back to their language, then to en-US.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateOrder {
    Mdy,
    Dmy,
    Ymd,
}

pub struct Culture {
    pub name: &'static str,
    pub decimal: char,
    pub group: char,
    pub order: DateOrder,
    /// .NET-style patterns.
    pub short_date: &'static str,
    pub long_date: &'static str,
    pub short_time: &'static str,
    pub long_time: &'static str,
    pub months: [&'static str; 12],
    /// Abbreviated month names; empty = the first three letters of the name.
    pub months_short: [&'static str; 12],
    /// Sunday first.
    pub days: [&'static str; 7],
    pub am: &'static str,
    pub pm: &'static str,
}

const EN_MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const EN_DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const PT_MONTHS: [&str; 12] =
    ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];
const PT_DAYS: [&str; 7] = ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"];
const ES_MONTHS: [&str; 12] =
    ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
const ES_DAYS: [&str; 7] = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];
const FR_MONTHS: [&str; 12] =
    ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];
const FR_MONTHS_SHORT: [&str; 12] = ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];
const FR_DAYS: [&str; 7] = ["dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi"];
const DE_MONTHS: [&str; 12] = ["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"];
const DE_DAYS: [&str; 7] = ["Sonntag", "Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag"];
const IT_MONTHS: [&str; 12] =
    ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"];
const IT_DAYS: [&str; 7] = ["domenica", "lunedì", "martedì", "mercoledì", "giovedì", "venerdì", "sabato"];
const NO_SHORT: [&str; 12] = [""; 12];

const fn culture(
    name: &'static str,
    decimal: char,
    group: char,
    order: DateOrder,
    patterns: [&'static str; 4],
    months: [&'static str; 12],
    months_short: [&'static str; 12],
    days: [&'static str; 7],
    am_pm: [&'static str; 2],
) -> Culture {
    Culture {
        name,
        decimal,
        group,
        order,
        short_date: patterns[0],
        long_date: patterns[1],
        short_time: patterns[2],
        long_time: patterns[3],
        months,
        months_short,
        days,
        am: am_pm[0],
        pm: am_pm[1],
    }
}

const H24: [&str; 2] = ["", ""];

pub static CULTURES: &[Culture] = &[
    culture(
        "en-US",
        '.',
        ',',
        DateOrder::Mdy,
        ["M/d/yyyy", "dddd, MMMM d, yyyy", "h:mm tt", "h:mm:ss tt"],
        EN_MONTHS,
        NO_SHORT,
        EN_DAYS,
        ["AM", "PM"],
    ),
    culture("en-GB", '.', ',', DateOrder::Dmy, ["dd/MM/yyyy", "dddd, d MMMM yyyy", "HH:mm", "HH:mm:ss"], EN_MONTHS, NO_SHORT, EN_DAYS, ["am", "pm"]),
    culture(
        "en-CA",
        '.',
        ',',
        DateOrder::Ymd,
        ["yyyy-MM-dd", "dddd, MMMM d, yyyy", "h:mm tt", "h:mm:ss tt"],
        EN_MONTHS,
        NO_SHORT,
        EN_DAYS,
        ["a.m.", "p.m."],
    ),
    culture(
        "en-AU",
        '.',
        ',',
        DateOrder::Dmy,
        ["d/MM/yyyy", "dddd, d MMMM yyyy", "h:mm tt", "h:mm:ss tt"],
        EN_MONTHS,
        NO_SHORT,
        EN_DAYS,
        ["am", "pm"],
    ),
    culture("pt-BR", ',', '.', DateOrder::Dmy, ["dd/MM/yyyy", "dddd, d 'de' MMMM 'de' yyyy", "HH:mm", "HH:mm:ss"], PT_MONTHS, NO_SHORT, PT_DAYS, H24),
    culture("pt-PT", ',', '\u{a0}', DateOrder::Dmy, ["dd/MM/yyyy", "d 'de' MMMM 'de' yyyy", "HH:mm", "HH:mm:ss"], PT_MONTHS, NO_SHORT, PT_DAYS, H24),
    culture("es-ES", ',', '.', DateOrder::Dmy, ["d/M/yyyy", "dddd, d 'de' MMMM 'de' yyyy", "H:mm", "H:mm:ss"], ES_MONTHS, NO_SHORT, ES_DAYS, H24),
    culture("es-MX", '.', ',', DateOrder::Dmy, ["dd/MM/yyyy", "dddd, d 'de' MMMM 'de' yyyy", "HH:mm", "HH:mm:ss"], ES_MONTHS, NO_SHORT, ES_DAYS, H24),
    culture(
        "fr-FR",
        ',',
        '\u{202f}',
        DateOrder::Dmy,
        ["dd/MM/yyyy", "dddd d MMMM yyyy", "HH:mm", "HH:mm:ss"],
        FR_MONTHS,
        FR_MONTHS_SHORT,
        FR_DAYS,
        H24,
    ),
    culture("de-DE", ',', '.', DateOrder::Dmy, ["dd.MM.yyyy", "dddd, d. MMMM yyyy", "HH:mm", "HH:mm:ss"], DE_MONTHS, NO_SHORT, DE_DAYS, H24),
    culture("it-IT", ',', '.', DateOrder::Dmy, ["dd/MM/yyyy", "dddd d MMMM yyyy", "HH:mm", "HH:mm:ss"], IT_MONTHS, NO_SHORT, IT_DAYS, H24),
    culture("ja-JP", '.', ',', DateOrder::Ymd, ["yyyy/MM/dd", "yyyy/MM/dd", "H:mm", "H:mm:ss"], EN_MONTHS, NO_SHORT, EN_DAYS, H24),
    culture("zh-CN", '.', ',', DateOrder::Ymd, ["yyyy/M/d", "yyyy/M/d", "H:mm", "H:mm:ss"], EN_MONTHS, NO_SHORT, EN_DAYS, H24),
    culture("sv-SE", ',', '\u{a0}', DateOrder::Ymd, ["yyyy-MM-dd", "yyyy-MM-dd", "HH:mm", "HH:mm:ss"], EN_MONTHS, NO_SHORT, EN_DAYS, H24),
];

/// The invariant culture (`""`): English names, month first, 24-hour clock.
pub static INVARIANT: Culture =
    culture("", '.', ',', DateOrder::Mdy, ["MM/dd/yyyy", "dddd, dd MMMM yyyy", "HH:mm", "HH:mm:ss"], EN_MONTHS, NO_SHORT, EN_DAYS, ["AM", "PM"]);

impl Culture {
    /// The culture called `name` (case-insensitive); otherwise the first culture of the same
    /// language; otherwise en-US.
    pub fn find(name: &str) -> &'static Culture {
        let name = name.trim();
        if name.is_empty() {
            return &INVARIANT;
        }
        if let Some(c) = CULTURES.iter().find(|c| c.name.eq_ignore_ascii_case(name)) {
            return c;
        }
        let lang = name.split(['-', '_']).next().unwrap_or(name);
        CULTURES
            .iter()
            .find(|c| c.name.split('-').next().is_some_and(|l| l.eq_ignore_ascii_case(lang)))
            .or_else(|| CULTURES.first())
            .unwrap_or(&INVARIANT)
    }

    /// The first day of the week, Sunday = 0.
    pub fn first_day_of_week(&self) -> i64 {
        match self.name {
            "en-US" | "en-CA" | "pt-BR" | "es-MX" | "ja-JP" | "zh-CN" | "" => 0,
            _ => 1,
        }
    }

    pub fn month_short(&self, m: usize) -> String {
        match self.months_short.get(m) {
            Some(s) if !s.is_empty() => (*s).to_string(),
            _ => self.months.get(m).map_or(String::new(), |s| s.chars().take(3).collect()),
        }
    }
    pub fn day_short(&self, d: usize) -> String {
        self.days.get(d).map_or(String::new(), |s| s.chars().take(3).collect())
    }

    /// Reads a number written in this culture: sign, digit grouping, decimals, exponent, a
    /// percent sign, a currency symbol, parentheses for negatives.
    pub fn parse_number(&self, text: &str) -> Option<f64> {
        let mut s = text.trim();
        match s {
            "NaN" => return Some(f64::NAN),
            "∞" | "Infinity" => return Some(f64::INFINITY),
            "-∞" | "-Infinity" => return Some(f64::NEG_INFINITY),
            "" => return None,
            _ => {}
        }
        let mut negative = false;
        if let Some(inner) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
            negative = true;
            s = inner.trim();
        }
        let mut percent = false;
        if let Some(r) = s.strip_suffix('%') {
            percent = true;
            s = r.trim_end();
        }
        // A sign may come before or after a currency symbol.
        for _ in 0..2 {
            if let Some(r) = s.strip_prefix('-').or_else(|| s.strip_prefix('\u{2212}')) {
                negative = !negative;
                s = r.trim_start();
            } else if let Some(r) = s.strip_prefix('+') {
                s = r.trim_start();
            }
            for sym in ["R$", "US$", "$", "€", "£", "¥"] {
                if let Some(r) = s.strip_prefix(sym) {
                    s = r.trim_start();
                } else if let Some(r) = s.strip_suffix(sym) {
                    s = r.trim_end();
                }
            }
        }
        if s.is_empty() || s.len() > 400 {
            return None;
        }
        let mut plain = String::with_capacity(s.len());
        let mut seen_decimal = false;
        let mut seen_exp = false;
        let mut digits = 0;
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '0'..='9' => {
                    digits += 1;
                    plain.push(c);
                }
                c if c == self.decimal && !seen_decimal && !seen_exp => {
                    seen_decimal = true;
                    plain.push('.');
                }
                // Grouping: between digits of the integer part. A space of any kind groups too.
                c if (c == self.group || (matches!(c, ' ' | '\u{a0}' | '\u{202f}') && self.group != '.' && self.group != ','))
                    && !seen_decimal
                    && !seen_exp
                    && digits > 0
                    && chars.peek().is_some_and(char::is_ascii_digit) => {}
                'e' | 'E' if !seen_exp && digits > 0 => {
                    seen_exp = true;
                    plain.push('e');
                    if let Some(sign) = chars.peek().copied().filter(|c| matches!(c, '+' | '-')) {
                        plain.push(sign);
                        chars.next();
                    }
                    if !chars.peek().is_some_and(char::is_ascii_digit) {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        if digits == 0 {
            return None;
        }
        let mut n: f64 = plain.parse().ok()?;
        if percent {
            n /= 100.0;
        }
        Some(if negative { -n } else { n })
    }

    /// A number as this culture writes it, with the shortest digits that read back the same.
    pub fn number_to_text(&self, n: f64) -> String {
        let s = crate::value::number_to_text(n);
        if self.decimal == '.' { s } else { s.replace('.', &self.decimal.to_string()) }
    }

    /// Fixed-point text with `decimals` places, grouped when `grouped`.
    pub fn fixed(&self, n: f64, decimals: usize, grouped: bool) -> String {
        let decimals = decimals.min(99);
        let s = format!("{:.*}", decimals, n.abs());
        let (int, frac) = s.split_once('.').unwrap_or((s.as_str(), ""));
        let mut out = String::new();
        // "-0.00" for a value that rounds to zero reads as plain zero.
        if n < 0.0 && s.bytes().any(|b| b.is_ascii_digit() && b != b'0') {
            out.push('-');
        }
        if grouped {
            let len = int.len();
            for (i, c) in int.chars().enumerate() {
                if i > 0 && (len - i) % 3 == 0 {
                    out.push(self.group);
                }
                out.push(c);
            }
        } else {
            out.push_str(int);
        }
        if !frac.is_empty() {
            out.push(self.decimal);
            out.push_str(frac);
        }
        out
    }
}
