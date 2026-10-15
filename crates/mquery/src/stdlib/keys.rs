//! Comparers and keys: how values are matched when grouping, de-duplicating, joining and
//! sorting.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use crate::datetime::utc_ticks;
use crate::error::{EResult, raise};
use crate::interp::Interp;
use crate::value::{Function, MAX_VALUE_DEPTH, Slot, Value, compare_values, values_equal};

/// How two values are compared.
#[derive(Clone)]
pub(crate) enum Comparer {
    /// By value, text by code point.
    Ordinal,
    /// By value, text without regard to case.
    IgnoreCase,
    /// A function of two values: it gives a number (below, at or above zero) or, for equality
    /// only, true or false.
    Custom(Value),
    /// A function of one value that gives the key to compare by.
    Key(Value),
}

impl Comparer {
    /// Reads a comparer or equation-criteria argument.
    pub(crate) fn from(v: &Value) -> EResult<Comparer> {
        match v {
            Value::Null => Ok(Comparer::Ordinal),
            Value::Function(f) => Ok(match (&**f, f.builtin_name()) {
                (_, Some("Comparer.Ordinal")) => Comparer::Ordinal,
                (_, Some("Comparer.OrdinalIgnoreCase")) => Comparer::IgnoreCase,
                (Function::Bound(_, bound), Some("Comparer.FromCulture")) => match bound.get(1) {
                    Some(Value::Logical(true)) => Comparer::IgnoreCase,
                    _ => Comparer::Ordinal,
                },
                _ if f.arity().1 == 1 => Comparer::Key(v.clone()),
                _ => Comparer::Custom(v.clone()),
            }),
            v => raise(format!("A comparer is expected, not a value of type {}.", v.kind())),
        }
    }

    pub(crate) fn ignore_case(&self) -> bool {
        matches!(self, Comparer::IgnoreCase)
    }

    /// Whether equal values can be found by hashing their keys.
    pub(crate) fn hashable(&self) -> bool {
        !matches!(self, Comparer::Custom(_))
    }

    pub(crate) fn equal(&self, it: &mut Interp, a: &Value, b: &Value) -> EResult<bool> {
        match self {
            Comparer::Ordinal => values_equal(a, b),
            Comparer::IgnoreCase => match (a, b) {
                (Value::Text(x), Value::Text(y)) => Ok(x.to_lowercase() == y.to_lowercase()),
                _ => values_equal(a, b),
            },
            Comparer::Custom(f) => match it.call(f, &[a.clone(), b.clone()])? {
                Value::Logical(b) => Ok(b),
                Value::Number(n) => Ok(n == 0.0),
                v => raise(format!("A comparer should give a number or true/false, not a value of type {}.", v.kind())),
            },
            Comparer::Key(f) => {
                let (x, y) = (it.call1(f, a.clone())?, it.call1(f, b.clone())?);
                values_equal(&x, &y)
            }
        }
    }

    pub(crate) fn compare(&self, it: &mut Interp, a: &Value, b: &Value) -> EResult<Ordering> {
        match self {
            Comparer::Ordinal => Ok(compare_values(a, b)),
            Comparer::IgnoreCase => Ok(match (a, b) {
                (Value::Text(x), Value::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
                _ => compare_values(a, b),
            }),
            Comparer::Custom(f) => match it.call(f, &[a.clone(), b.clone()])? {
                Value::Number(n) => Ok(n.partial_cmp(&0.0).unwrap_or(Ordering::Equal)),
                v => raise(format!("A comparer should give a number, not a value of type {}.", v.kind())),
            },
            Comparer::Key(f) => {
                let (x, y) = (it.call1(f, a.clone())?, it.call1(f, b.clone())?);
                Ok(compare_values(&x, &y))
            }
        }
    }

    /// The bytes that are the same exactly for values this comparer finds equal.
    pub(crate) fn key(&self, it: &mut Interp, s: &Slot, out: &mut Vec<u8>) -> EResult<()> {
        match (self, s) {
            (Comparer::Key(f), Slot::Val(v)) => {
                let k = it.call1(f, v.clone())?;
                encode(&k, false, out, 0);
            }
            _ => encode_slot(s, self.ignore_case(), out, 0),
        }
        Ok(())
    }
}

fn encode_slot(s: &Slot, ignore_case: bool, out: &mut Vec<u8>, depth: usize) {
    match s.result() {
        Ok(v) => encode(&v, ignore_case, out, depth),
        Err(e) => {
            out.push(15);
            push_bytes(out, e.message.as_bytes());
        }
    }
}

fn push_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}

/// A canonical encoding: equal values give equal bytes.
fn encode(v: &Value, ignore_case: bool, out: &mut Vec<u8>, depth: usize) {
    if depth > MAX_VALUE_DEPTH {
        out.push(255);
        return;
    }
    match v {
        Value::Null => out.push(0),
        Value::Logical(b) => out.extend_from_slice(&[1, *b as u8]),
        Value::Number(n) => {
            out.push(2);
            // 0 and -0 are equal; every NaN shares one key.
            let n = if *n == 0.0 {
                0.0
            } else if n.is_nan() {
                f64::NAN
            } else {
                *n
            };
            out.extend_from_slice(&n.to_bits().to_le_bytes());
        }
        Value::Text(s) => {
            out.push(3);
            if ignore_case {
                push_bytes(out, s.to_lowercase().as_bytes());
            } else {
                push_bytes(out, s.as_bytes());
            }
        }
        Value::Date(d) => {
            out.push(4);
            out.extend_from_slice(&d.to_le_bytes());
        }
        Value::Time(t) => {
            out.push(5);
            out.extend_from_slice(&t.to_le_bytes());
        }
        Value::DateTime(t) => {
            out.push(6);
            out.extend_from_slice(&t.to_le_bytes());
        }
        Value::DateTimeZone(t, off) => {
            out.push(7);
            out.extend_from_slice(&utc_ticks(*t, *off).to_le_bytes());
        }
        Value::Duration(t) => {
            out.push(8);
            out.extend_from_slice(&t.to_le_bytes());
        }
        Value::Binary(b) => {
            out.push(9);
            push_bytes(out, b);
        }
        Value::List(l) => {
            out.push(10);
            out.extend_from_slice(&(l.len() as u64).to_le_bytes());
            for s in &l.items {
                encode_slot(s, ignore_case, out, depth + 1);
            }
        }
        Value::Record(r) => {
            // Field order doesn't matter to equality: encode by name.
            out.push(11);
            let mut order: Vec<usize> = (0..r.len()).collect();
            order.sort_by(|a, b| r.names.get(*a).cmp(&r.names.get(*b)));
            out.extend_from_slice(&(order.len() as u64).to_le_bytes());
            for i in order {
                if let (Some(n), Some(s)) = (r.names.get(i), r.values.get(i)) {
                    push_bytes(out, n.as_bytes());
                    encode_slot(s, ignore_case, out, depth + 1);
                }
            }
        }
        Value::Table(t) => {
            out.push(12);
            out.extend_from_slice(&(t.width() as u64).to_le_bytes());
            for c in t.columns.iter() {
                push_bytes(out, c.as_bytes());
            }
            for row in &t.rows {
                for s in row.iter() {
                    encode_slot(s, ignore_case, out, depth + 1);
                }
            }
        }
        Value::Function(f) => {
            out.push(13);
            out.extend_from_slice(&(Rc::as_ptr(f) as usize as u64).to_le_bytes());
        }
        Value::Type(t) => {
            out.push(14);
            push_bytes(out, t.name().as_bytes());
        }
    }
}

/// Groups positions by key, in order of first appearance: the tool behind distinct, group and
/// join. With a custom comparer, where keys can't be hashed, it compares against each group.
pub(crate) struct Grouper {
    comparer: Comparer,
    index: HashMap<Vec<u8>, usize>,
    /// One representative key per group, for comparers that need the values.
    samples: Vec<Vec<Value>>,
    scratch: Vec<u8>,
}

impl Grouper {
    pub(crate) fn new(comparer: Comparer) -> Grouper {
        Grouper { comparer, index: HashMap::new(), samples: Vec::new(), scratch: Vec::new() }
    }

    pub(crate) fn len(&self) -> usize {
        if self.comparer.hashable() { self.index.len() } else { self.samples.len() }
    }

    /// The group of `key`, and whether it is new.
    pub(crate) fn group(&mut self, it: &mut Interp, key: &[&Slot]) -> EResult<(usize, bool)> {
        match self.find(it, key)? {
            Some(g) => Ok((g, false)),
            None => {
                let g = self.len();
                if self.comparer.hashable() {
                    self.index.insert(std::mem::take(&mut self.scratch), g);
                } else {
                    self.samples.push(key.iter().map(|s| s.get()).collect::<EResult<Vec<Value>>>()?);
                }
                Ok((g, true))
            }
        }
    }

    /// The group of `key`, if it has been seen.
    pub(crate) fn find(&mut self, it: &mut Interp, key: &[&Slot]) -> EResult<Option<usize>> {
        if self.comparer.hashable() {
            self.scratch.clear();
            for s in key {
                self.comparer.key(it, s, &mut self.scratch)?;
            }
            return Ok(self.index.get(&self.scratch).copied());
        }
        let values = key.iter().map(|s| s.get()).collect::<EResult<Vec<Value>>>()?;
        'groups: for (g, sample) in self.samples.iter().enumerate() {
            for (x, y) in sample.iter().zip(&values) {
                if !self.comparer.equal(it, x, y)? {
                    continue 'groups;
                }
            }
            return Ok(Some(g));
        }
        Ok(None)
    }
}

/// Sorts `items` by `compare`, which may fail: the first failure is returned. The sort is stable.
///
/// A merge sort of our own rather than the standard one, which may panic when the order isn't
/// consistent, as an order given by a query's own comparer function need not be.
pub(crate) fn try_sort_by<T>(items: Vec<T>, mut compare: impl FnMut(&T, &T) -> EResult<Ordering>) -> EResult<Vec<T>> {
    let n = items.len();
    if n < 2 {
        return Ok(items);
    }
    let mut order: Vec<usize> = (0..n).collect();
    let mut merged: Vec<usize> = Vec::with_capacity(n);
    let mut width = 1;
    while width < n {
        merged.clear();
        for run in order.chunks(width.saturating_mul(2)) {
            let (left, right) = run.split_at_checked(width).unwrap_or((run, &[]));
            let (mut l, mut r) = (left.iter().copied().peekable(), right.iter().copied().peekable());
            loop {
                match (l.peek().copied(), r.peek().copied()) {
                    (Some(a), Some(b)) => {
                        let less = match (items.get(b), items.get(a)) {
                            (Some(x), Some(y)) => compare(x, y)? == Ordering::Less,
                            _ => false,
                        };
                        if less {
                            merged.push(b);
                            r.next();
                        } else {
                            merged.push(a);
                            l.next();
                        }
                    }
                    (Some(_), None) => {
                        merged.extend(l);
                        break;
                    }
                    (None, _) => {
                        merged.extend(r);
                        break;
                    }
                }
            }
        }
        std::mem::swap(&mut order, &mut merged);
        width = width.saturating_mul(2);
    }
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    Ok(order.into_iter().filter_map(|i| slots.get_mut(i).and_then(Option::take)).collect())
}
