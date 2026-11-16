//! Field format pictures (#267): the grammar behind a Field row's
//! `Format` cell — the string FIELDPICTURE(n) produces and FORMAT()
//! consumes — applied to an evaluated field value.
//!
//! Provenance: the picture INVENTORY and the FIELDPICTURE(n)→picture
//! table are pinned from the shipped stencil library's own cached
//! outputs (2026-11-16 census over `Sample/Office16/Visio Content`,
//! 679 packages — `F='FIELDPICTURE(n)'` beside its computed `V`), and
//! the rendering anchors from the #223 parity review (MisterDiagram,
//! measured): **`esc(0)` = general format, no unit suffix** (numbers
//! minimal-form, strings verbatim) and **`esc(8)` = three decimals,
//! no units** — both contradicting the MS page's example ("one
//! decimal place and a lowercase unit description"), which the
//! measurements supersede. The `esc(n)` decimal slope for n in
//! 2..=9 (`decimals = 11 - n`, putting the measured 8 at three
//! decimals and giving every code a distinct fixed format) is the
//! inferred reading of the issue's "decimal pictures 2–9" note
//! consistent with both measured anchors; `esc(1)` = general WITH a lowercase unit
//! suffix is the natural counterpart to esc(0) (the census rows
//! carry raw internal-inch values).
//!
//! Dates render from either encoding the corpus saves: a serial day
//! number (Excel/Visio epoch 1899-12-30, e.g. `42005`) or an ISO
//! `YYYY-MM-DD[T HH:MM[:SS]]` string. Unknown pictures fall back to
//! the general form — a field that cannot be formatted still renders.

/// Render `value` (a number or string, as `Value` carries it) through
/// `pic`. `value_is_str` distinguishes the two `Value` variants for
/// the `@` family (a string passes through verbatim; a number gets
/// the general form).
pub fn apply_picture(value_num: Option<f64>, value_str: Option<&str>, pic: &str) -> String {
    let general = || match (value_num, value_str) {
        (Some(n), _) => general_num(n),
        (None, Some(s)) => s.to_string(),
        _ => String::new(),
    };
    let p = pic.trim();
    if let Some(inner) = p
        .strip_prefix("{{")
        .and_then(|r| r.strip_suffix("}}"))
    {
        return date_picture(value_num, value_str, inner);
    }
    if p.starts_with("esc(") && p.ends_with(')') {
        let n: i64 = p[4..p.len() - 1].trim().parse().unwrap_or(0);
        return esc_picture(n, value_num, value_str);
    }
    if p == "@" || p == "@+" {
        // String verbatim; a numeric value takes the general form.
        return value_str.map(str::to_string).unwrap_or_else(general);
    }
    if p == "0" || p == "1" {
        return value_num.map(|n| format!("{n:.0}")).unwrap_or_else(general);
    }
    if let Some(rest) = p.strip_suffix('u') {
        // 0u / 0.00u: fixed decimals + a lowercase unit suffix. The
        // field's value is internal inches.
        if let Some(n) = value_num {
            let decimals = rest.split_once('.').map_or(0, |(_, d)| d.len());
            return format!("{:.*} in", decimals, n);
        }
        return general();
    }
    if p == "c" || p.starts_with("{<") || p == "ddddd" {
        // Date/time general: locale-style short date, plus a time of
        // day when the value carries one.
        return datetime_general(value_num, value_str);
    }
    // FORMAT's callers also pass plain .NET-ish codes (`"d"` = short
    // date is corpus-attested); anything unrecognized renders general.
    match p {
        "d" => date_picture(value_num, value_str, "M/d/yyyy"),
        _ => general(),
    }
}

/// The `esc(n)` family. Anchors: esc(0) general/no units (measured),
/// esc(8) three decimals/no units (measured); 2..=9 slope = n - 5
/// (inferred, consistent with both); esc(1) general + unit;
/// everything else falls back to general.
fn esc_picture(n: i64, value_num: Option<f64>, value_str: Option<&str>) -> String {
    let general = || match (value_num, value_str) {
        (Some(v), _) => general_num(v),
        (None, Some(s)) => s.to_string(),
        _ => String::new(),
    };
    match n {
        0 => general(),
        1 => match value_num {
            Some(v) => format!("{} in", general_num(v)),
            None => general(),
        },
        d @ 2..=9 => {
            // The inferred slope: 11 - n puts the measured esc(8) at
            // three decimals and gives every code 2..=9 a distinct
            // fixed format (9 dp down to 2 dp).
            let decimals = (11 - d).clamp(0, 9) as usize;
            match value_num {
                Some(v) => format!("{v:.decimals$}"),
                None => general(),
            }
        }
        _ => general(),
    }
}

/// Visio's general number form: minimal digits — integers plain, the
/// rest shortest-roundtrip (the cached corpus values are stored in
/// this shape).
fn general_num(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// The `{{...}}` date-picture family (corpus: `M/d/yyyy`, `M/d`,
/// `MMM yyyy`, `MMM-yy`, `MMMM d, yyyy`, `d MMMM yyyy`). Tokens:
/// `yyyy` `yy` `MMMM` `MMM` `MM` `M` `dddd`? no — `dd` `d`, everything
/// else literal.
fn date_picture(value_num: Option<f64>, value_str: Option<&str>, pic: &str) -> String {
    let Some(d) = as_datetime(value_num, value_str) else {
        return String::new();
    };
    let mut out = String::new();
    let mut i = 0;
    'outer: while i < pic.len() {
        let rest = &pic[i..];
        let short_month = match d.month {
            1 => "Jan",
            2 => "Feb",
            3 => "Mar",
            4 => "Apr",
            5 => "May",
            6 => "Jun",
            7 => "Jul",
            8 => "Aug",
            9 => "Sep",
            10 => "Oct",
            11 => "Nov",
            _ => "Dec",
        };
        let tokens = [
            ("yyyy", format!("{:04}", d.year)),
            ("yy", format!("{:02}", d.year % 100)),
            ("MMMM", month_name(d.month).to_string()),
            ("MMM", short_month.to_string()),
            ("MM", format!("{:02}", d.month)),
            ("M", d.month.to_string()),
            ("dd", format!("{:02}", d.day)),
            ("d", d.day.to_string()),
        ];
        for (tok, val) in tokens {
            if rest.starts_with(tok) {
                out.push_str(&val);
                i += tok.len();
                continue 'outer;
            }
        }
        // No token here: the current character is a literal. Multi-byte
        // characters step one char at a time.
        let ch_len = rest.chars().next().map_or(1, char::len_utf8);
        out.push_str(&rest[..ch_len]);
        i += ch_len;
    }
    out
}

/// Date/time general (`c`, `{<n>}`, `ddddd`): `M/d/yyyy`, plus
/// ` h:mm AM/PM` when the value carries a time of day.
fn datetime_general(value_num: Option<f64>, value_str: Option<&str>) -> String {
    let Some(d) = as_datetime(value_num, value_str) else {
        return String::new();
    };
    let mut out = format!("{}/{}/{}", d.month, d.day, d.year);
    if d.has_time {
        let (h12, ampm) = match d.hour {
            0 => (12, "AM"),
            h @ 1..=11 => (h, "AM"),
            12 => (12, "PM"),
            h => (h - 12, "PM"),
        };
        out.push_str(&format!(" {h12}:{:02} {ampm}", d.minute));
    }
    out
}

struct DateTime {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    has_time: bool,
}

/// Accept both corpus encodings: a serial day number (Excel/Visio
/// epoch 1899-12-30; the fraction of a day is the time) or an ISO
/// `YYYY-MM-DD[T ]HH:MM[:SS]` string.
fn as_datetime(value_num: Option<f64>, value_str: Option<&str>) -> Option<DateTime> {
    if let Some(n) = value_num {
        let days = n.floor() as i64;
        let frac = n - n.floor();
        let (year, month, day) = civil_from_days(days);
        let minutes = (frac * 1440.0).round() as i64;
        let has_time = minutes != 0;
        return Some(DateTime {
            year,
            month,
            day,
            hour: (minutes / 60).rem_euclid(24) as u32,
            minute: (minutes % 60).rem_euclid(60) as u32,
            has_time,
        });
    }
    let s = value_str?.trim();
    // ISO date, optional T/space + time.
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let mut dp = date.split('-');
    let year: i64 = dp.next()?.parse().ok()?;
    let month: u32 = dp.next()?.parse().ok()?;
    let day: u32 = dp.next()?.parse().ok()?;
    let (mut hour, mut minute) = (0, 0);
    let mut has_time = false;
    if let Some(t) = time {
        let mut tp = t.split(':');
        hour = tp.next()?.parse().ok()?;
        minute = tp.next().unwrap_or("0").parse().ok()?;
        let seconds: u32 = tp.next().unwrap_or("0").parse().unwrap_or(0);
        // A T00:00:00 stamp is a DATE saved with a zero time — Visio
        // renders it date-only.
        has_time = hour != 0 || minute != 0 || seconds != 0;
    }
    Some(DateTime {
        year,
        month,
        day,
        hour,
        minute,
        has_time,
    })
}

/// Days since 1899-12-30 (the Excel/Visio serial epoch) to a civil
/// date — Howard Hinnant's `civil_from_days` algorithm, no chrono
/// dependency. 42005 = 2015-01-01 and 41640 = 2014-01-01 pin the
/// epoch against the corpus's cached serials (25 569 days separate
/// 1899-12-30 from 1970-01-01).
fn civil_from_days(serial: i64) -> (i64, u32, u32) {
    let z = serial - 25_569 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since the serial epoch for a civil date (`DATE(y,m,d)`'s
/// result) — the inverse of [`civil_from_days`].
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    doe - 719_468 + era * 146_097 + 25_569
}

fn month_name(m: u32) -> &'static str {
    match m {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        _ => "December",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: Option<f64> = None;
    const NS: Option<&str> = None;
    fn n(v: f64) -> Option<f64> {
        Some(v)
    }

    /// The two measured anchors from the #223 parity review (they
    /// supersede the MS page's esc(0) example).
    #[test]
    fn esc_anchors_from_the_parity_review() {
        // esc(0) = general, NO unit suffix (MisterDiagram's witness:
        // FIELDPICTURE(8) width renders "2", and the corpus caches
        // bare strings "30", "1", "Interval Description").
        assert_eq!(apply_picture(n(2.0), NS, "esc(0)"), "2");
        assert_eq!(apply_picture(n(165.3543), NS, "esc(0)"), "165.3543");
        assert_eq!(apply_picture(N, Some("Interval Description"), "esc(0)"), "Interval Description");
        // esc(8) = three decimals, no units (measured).
        assert_eq!(apply_picture(n(2.0), NS, "esc(8)"), "2.000");
        assert_eq!(apply_picture(n(0.13338282470703125), NS, "esc(8)"), "0.133");
    }

    /// The inferred decimal slope: esc(n) = n-5 decimals for 2..=9
    /// (puts the measured esc(8) at 3). Cached corpus values stay
    /// inside the picture's digits: esc(3) = 1 dp, esc(7) = 2 dp.
    #[test]
    fn esc_decimal_slope() {
        assert_eq!(apply_picture(n(0.25), NS, "esc(3)"), "0.25000000");
        assert_eq!(apply_picture(n(165.3543307086614), NS, "esc(5)"), "165.354331");
        assert_eq!(apply_picture(n(118.1102362204725), NS, "esc(7)"), "118.1102");
    }

    #[test]
    fn string_pictures_pass_through() {
        assert_eq!(apply_picture(N, Some("12.50"), "@"), "12.50");
        assert_eq!(apply_picture(N, Some("Microsoft Corporation"), "@+"), "Microsoft Corporation");
        // A numeric value under @ takes the general form.
        assert_eq!(apply_picture(n(2.5), None, "@"), "2.5");
    }

    #[test]
    fn unit_pictures() {
        assert_eq!(apply_picture(n(118.1102362204724), NS, "0u"), "118 in");
        assert_eq!(apply_picture(n(1.968503937007874), NS, "0.00u"), "1.97 in");
        assert_eq!(apply_picture(n(1.181102362204724), NS, "esc(1)"), "1.181102362204724 in");
    }

    #[test]
    fn date_serial_pictures() {
        // Corpus anchors: 42005 with `{{d MMMM yyyy}}` and 41640 with
        // `{{MMMM d, yyyy}}`.
        assert_eq!(apply_picture(n(42005.0), NS, "{{d MMMM yyyy}}"), "1 January 2015");
        assert_eq!(apply_picture(n(41640.0), NS, "{{MMMM d, yyyy}}"), "January 1, 2014");
        assert_eq!(apply_picture(n(42043.0), NS, "{{M/d/yyyy}}"), "2/8/2015");
        assert_eq!(apply_picture(n(42095.0), NS, "{{MMM yyyy}}"), "Apr 2015");
    }

    #[test]
    fn date_iso_string_pictures() {
        assert_eq!(
            apply_picture(N, Some("2014-05-15T00:00:00"), "c"),
            "5/15/2014"
        );
        assert_eq!(
            apply_picture(N, Some("2012-08-01T19:47:37"), "{<200>}"),
            "8/1/2012 7:47 PM"
        );
        assert_eq!(apply_picture(n(42043.0), NS, "c"), "2/8/2015");
    }

    #[test]
    fn zero_decimals_and_format_codes() {
        assert_eq!(apply_picture(n(0.0), NS, "0"), "0");
        // FORMAT's "d" = short date (corpus-attested); serial 12 is
        // 1900-01-11 in the Excel epoch, date-only (the "d" code
        // carries no time).
        assert_eq!(apply_picture(n(12.5), NS, "d"), "1/11/1900");
    }

    #[test]
    fn serial_roundtrip() {
        for serial in [42005, 41640, 42043, 1, 60, 44927] {
            let (y, m, d) = civil_from_days(serial);
            assert_eq!(days_from_civil(y, m as i64, d as i64), serial, "{serial}");
        }
    }
}
