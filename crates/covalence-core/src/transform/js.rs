//! The few JavaScript rules the native binding kinds need, so they give the
//! same result as their compiled expressions (ADR 0023): `String(v)`,
//! `Number(v)`, `toFixed`, `Math.round`, `===` and `<`.
//!
//! Values are JSON, as they reach JavaScript ([`crate::expr::to_json`]);
//! `None` is `undefined`.

use std::cmp::Ordering;

use serde_json::Value;

/// `String(v)`.
pub fn to_string(v: Option<&Value>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => number_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        // `Array.prototype.join`: `null` and `undefined` items are empty.
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => to_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// `Number.prototype.toString()`: the shortest digits that round-trip,
/// placed the way ECMAScript's `Number::toString` places them.
pub fn number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x == 0.0 {
        return "0".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let sign = if x < 0.0 { "-" } else { "" };
    // Rust's `{:e}` gives the shortest round-trip digits: `1.2345e-7`.
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').expect("`{:e}` has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp.parse::<i32>().expect("`{:e}` exponents are integers") + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e = if e < 0 {
            format!("-{}", -e)
        } else {
            format!("+{e}")
        };
        if k == 1 {
            format!("{digits}e{e}")
        } else {
            format!("{}.{}e{e}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

/// `Number(v)`.
pub fn to_number(v: Option<&Value>) -> f64 {
    match v {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => string_to_number(s),
        // Through the primitive: `[]` is "", `[5]` is "5", objects are NaN.
        Some(other @ (Value::Array(_) | Value::Object(_))) => {
            string_to_number(&to_string(Some(other)))
        }
    }
}

/// ECMAScript's whitespace and line terminators, which `Number("…")` trims.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `Number("…")`: decimal literals, `Infinity`, and unsigned `0x`, `0o`
/// and `0b` integers. Anything else is NaN; an empty string is 0.
fn string_to_number(s: &str) -> f64 {
    let s = s.trim_matches(is_js_space);
    if s.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = s.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return digits.chars().fold(0.0, |acc, c| {
                acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
            });
        }
    }
    let (sign, rest) = match s.as_bytes()[0] {
        b'+' => (1.0, &s[1..]),
        b'-' => (-1.0, &s[1..]),
        _ => (1.0, s),
    };
    if rest == "Infinity" {
        return sign * f64::INFINITY;
    }
    if !is_decimal_literal(rest) {
        return f64::NAN;
    }
    rest.parse::<f64>().map_or(f64::NAN, |x| sign * x)
}

/// `1`, `1.`, `1.5`, `.5`, each with an optional `e±n`. Rust's float parser
/// also takes `inf` and `nan`, which JavaScript doesn't.
fn is_decimal_literal(s: &str) -> bool {
    let (number, exponent) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let digits = |t: &str| t.bytes().all(|b| b.is_ascii_digit());
    let mantissa = digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    mantissa && exponent_ok
}

/// `Math.round`: halves round towards +∞.
pub fn round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let floor = x.floor();
    let r = if x - floor >= 0.5 { floor + 1.0 } else { floor };
    // `Math.round(-0.4)` is -0.
    if r == 0.0 && x < 0.0 { -0.0 } else { r }
}

/// `Number.prototype.toFixed(digits)`, for `digits` up to 20.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.abs() >= 1e21 {
        return number_to_string(x);
    }
    let sign = if x < 0.0 { "-" } else { "" };
    // The exact decimal value, then rounded half up: JavaScript picks the
    // larger `n` on a tie, where Rust's own rounding picks the even one.
    let exact = format!("{:.1100}", x.abs());
    let (whole, fraction) = exact.split_once('.').expect("a fixed format has a point");
    let mut kept: Vec<u8> = whole.bytes().chain(fraction.bytes().take(digits)).collect();
    if fraction.as_bytes()[digits] >= b'5' {
        let mut i = kept.len();
        loop {
            if i == 0 {
                kept.insert(0, b'1');
                break;
            }
            i -= 1;
            if kept[i] == b'9' {
                kept[i] = b'0';
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let point = kept.len() - digits;
    let text = String::from_utf8(kept).expect("ASCII digits");
    if digits == 0 {
        format!("{sign}{text}")
    } else {
        format!("{sign}{}.{}", &text[..point], &text[point..])
    }
}

/// `Math.min` and `Math.max`, which return NaN if either argument is NaN.
pub fn min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

pub fn max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `a === b`. Objects and arrays are never equal: state reaches an
/// expression as fresh objects.
pub fn strict_equals(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        _ => false,
    }
}

/// The order of `a` and `b` as `<` sees it: strings by UTF-16 code unit when
/// both are strings, numbers otherwise. `None` when either is NaN.
pub fn compare(a: Option<&Value>, b: Option<&Value>) -> Option<Ordering> {
    let primitive = |v: Option<&Value>| match v {
        Some(object @ (Value::Array(_) | Value::Object(_))) => {
            Some(Value::String(to_string(Some(object))))
        }
        other => other.cloned(),
    };
    let (a, b) = (primitive(a), primitive(b));
    if let (Some(Value::String(x)), Some(Value::String(y))) = (&a, &b) {
        return Some(x.encode_utf16().cmp(y.encode_utf16()));
    }
    to_number(a.as_ref()).partial_cmp(&to_number(b.as_ref()))
}

/// The standard library's `mmss` and `hhmmss` (`expr/prelude.js`).
pub fn clock(v: Option<&Value>, with_hours: bool) -> String {
    let seconds = to_number(v);
    let sign = if seconds < 0.0 { "-" } else { "" };
    let mut s = seconds.abs().floor();
    let h = (s / 3600.0).floor();
    let m = if with_hours {
        ((s % 3600.0) / 60.0).floor()
    } else {
        (s / 60.0).floor()
    };
    s %= 60.0;
    let two = |n: f64| format!("{:0>2}", number_to_string(n));
    if with_hours {
        format!("{sign}{}:{}:{}", two(h), two(m), two(s))
    } else {
        format!("{sign}{}:{}", two(m), two(s))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn numbers_print_as_javascript_prints_them() {
        for (x, text) in [
            (0.0, "0"),
            (-0.0, "0"),
            (3.0, "3"),
            (-1.5, "-1.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e21, "1e+21"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (2.5e25, "2.5e+25"),
            (f64::NAN, "NaN"),
            (f64::NEG_INFINITY, "-Infinity"),
        ] {
            assert_eq!(number_to_string(x), text, "{x}");
        }
    }

    #[test]
    fn strings_become_numbers_as_in_javascript() {
        for (s, x) in [
            ("", 0.0),
            ("  12 ", 12.0),
            ("1e3", 1000.0),
            (".5", 0.5),
            ("5.", 5.0),
            ("-Infinity", f64::NEG_INFINITY),
            ("0x1F", 31.0),
            ("0b101", 5.0),
        ] {
            assert_eq!(to_number(Some(&json!(s))), x, "{s:?}");
        }
        for s in ["inf", "nan", "NaN", "1,5", "-0x1", "12px", "e5", "1e", "."] {
            assert!(to_number(Some(&json!(s))).is_nan(), "{s:?}");
        }
        assert_eq!(to_number(Some(&json!([]))), 0.0);
        assert_eq!(to_number(Some(&json!([7]))), 7.0);
        assert!(to_number(Some(&json!({}))).is_nan());
        assert!(to_number(None).is_nan());
    }

    #[test]
    fn to_fixed_rounds_ties_up() {
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.005, 2), "1.00", "1.005 is a little below");
        assert_eq!(to_fixed(-0.001, 2), "-0.00");
        assert_eq!(to_fixed(-0.0, 1), "0.0");
        assert_eq!(to_fixed(9.96, 1), "10.0");
        assert_eq!(to_fixed(1e21, 2), "1e+21");
    }

    #[test]
    fn round_and_compare_follow_javascript() {
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(-2.5), -2.0);
        assert_eq!(round(0.49999999999999994), 0.0);
        assert!(round(-0.4).is_sign_negative());
        assert_eq!(
            compare(Some(&json!("10")), Some(&json!(9))),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare(Some(&json!("10")), Some(&json!("9"))),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare(Some(&json!(null)), Some(&json!(0))),
            Some(Ordering::Equal)
        );
        assert_eq!(compare(None, Some(&json!(0))), None);
        assert!(strict_equals(Some(&json!(3)), Some(&json!(3.0))));
        assert!(!strict_equals(Some(&json!(3)), Some(&json!("3"))));
        assert!(!strict_equals(Some(&json!([1])), Some(&json!([1]))));
    }

    #[test]
    fn clocks_match_the_standard_library() {
        assert_eq!(clock(Some(&json!(299.7)), false), "04:59");
        assert_eq!(clock(Some(&json!(-61)), false), "-01:01");
        assert_eq!(clock(Some(&json!(3725)), true), "01:02:05");
        assert_eq!(clock(Some(&json!(6000)), false), "100:00");
        assert_eq!(clock(None, false), "NaN:NaN");
    }
}
