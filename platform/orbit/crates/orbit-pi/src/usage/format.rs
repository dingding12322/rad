//! Number and time formatting for the usage page.
//!
//! Two registers, used consistently (§54/§55/§84):
//!
//! - **Compact** in the main UI — `842`, `12.4K`, `1.28M`, `18.4M`.
//! - **Exact** in tooltips and detail tables — `18,421,842`.
//!
//! Nothing here is allowed to round a value into a lie: a compacted figure
//! always keeps three significant digits, and a value that cannot be known is
//! rendered by the view as words, not as a number.

/// Compact token/count figure: `842`, `12.4K`, `1.28M`, `18.4M`, `2.1B`.
pub fn compact(n: u64) -> String {
    let value = n as f64;
    if n < 1_000 {
        return n.to_string();
    }
    if value < 1_000_000.0 {
        return scaled(value / 1_000.0, 'K');
    }
    if value < 1_000_000_000.0 {
        return scaled(value / 1_000_000.0, 'M');
    }
    scaled(value / 1_000_000_000.0, 'B')
}

/// Same as [`compact`] — named for the places where the unit matters.
pub fn compact_tokens(n: u64) -> String {
    compact(n)
}

/// Three significant digits, trailing zeros trimmed: `1.28M`, `18.4M`, `842K`.
fn scaled(value: f64, unit: char) -> String {
    let text = if value < 10.0 {
        format!("{value:.2}")
    } else if value < 100.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    };
    let trimmed = match text.split_once('.') {
        Some((whole, decimals)) => {
            let decimals = decimals.trim_end_matches('0');
            if decimals.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{decimals}")
            }
        }
        None => text,
    };
    format!("{trimmed}{unit}")
}

/// Exact figure with thousands separators: `18,421,842`.
pub fn exact(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, ch) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Counts in tables: exact while it stays readable, compact after that.
pub fn count(n: u64) -> String {
    if n < 10_000 {
        exact(n)
    } else {
        compact(n)
    }
}

/// USD amount: `<$0.01`, `$0.84`, `$12.40`, `$1,284.20`.
pub fn cost(usd: f64) -> String {
    if !usd.is_finite() {
        return "—".into();
    }
    if usd <= 0.0 {
        return "$0.00".into();
    }
    if usd < 0.01 {
        return "<$0.01".into();
    }
    if usd < 1_000.0 {
        return format!("${usd:.2}");
    }
    let whole = usd.trunc();
    let cents = (usd - whole) * 100.0;
    format!("${}.{:02}", exact(whole as u64), cents.round() as u64)
}

/// Shorten an absolute path for prose: `~/.pi/agent/sessions`.
pub fn short_path(path: &str) -> String {
    // The raw home string, not a canonicalized one: on Windows
    // `canonicalize` answers with a `\\?\` verbatim path that would never
    // prefix-match the plain paths pi reports.
    let home = crate::platform::home_dir();
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some(rest) => format!("~{rest}"),
        None => path.to_string(),
    }
}

/// Percentage with the precision the magnitude deserves: `68.4%`, `8.2%`,
/// `52%`, `0.42%`.
pub fn percent(value: f64) -> String {
    if !value.is_finite() {
        return "—".into();
    }
    let magnitude = value.abs();
    if magnitude > 0.0 && magnitude < 0.01 {
        return "<0.01%".into();
    }
    if magnitude < 1.0 {
        // Small rates live or die on their second decimal.
        return format!("{}%", trim_zeros(&format!("{value:.2}")));
    }
    // A share below the whole must never round up to a claimed 100%.
    if value < 100.0 && format!("{value:.1}") == "100.0" {
        return "99.9%".into();
    }
    format!("{}%", trim_zeros(&format!("{value:.1}")))
}

/// Drop a trailing `.0` so a whole number reads as one: `52.0` → `52`.
fn trim_zeros(text: &str) -> String {
    match text.split_once('.') {
        Some((whole, decimals)) => {
            let decimals = decimals.trim_end_matches('0');
            if decimals.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{decimals}")
            }
        }
        None => text.to_string(),
    }
}

/// A 0..=1 share as a percentage.
pub fn share(fraction: f64) -> String {
    percent(fraction * 100.0)
}

/// A signed percentage for a comparison: `+12.4%`, `-8.2%`, `no change`.
pub fn delta(pct: Option<f64>) -> String {
    match pct {
        None => tr!("usage.no_change"),
        Some(pct) if pct.abs() < 0.05 => tr!("usage.no_change"),
        Some(pct) => format!("{:+.1}%", pct),
    }
}

/// A duration in milliseconds at the scale it deserves: `420ms`, `1.8s`,
/// `42s`, `2m 14s`, `2h 14m`.
pub fn duration_ms(ms: f64) -> String {
    if !ms.is_finite() || ms < 0.0 {
        return "—".into();
    }
    if ms < 1_000.0 {
        format!("{:.0}ms", ms)
    } else if ms < 10_000.0 {
        format!("{:.1}s", ms / 1_000.0)
    } else if ms < 60_000.0 {
        format!("{:.0}s", ms / 1_000.0)
    } else if ms < 3_600_000.0 {
        format!(
            "{}m {}s",
            (ms / 60_000.0) as u64,
            (ms / 1_000.0) as u64 % 60
        )
    } else {
        format!(
            "{}h {:02}m",
            (ms / 3_600_000.0) as u64,
            (ms / 60_000.0) as u64 % 60
        )
    }
}

/// A wall-clock span (session length): `42s`, `8m 42s`, `2h 14m`, `3d 4h`.
pub fn span_ms(ms: i64) -> String {
    if ms <= 0 {
        return "—".into();
    }
    let secs = ms as f64 / 1_000.0;
    if secs < 60.0 {
        format!("{:.0}s", secs)
    } else if secs < 3_600.0 {
        format!("{}m {:.0}s", (secs / 60.0) as u64, secs % 60.0)
    } else if secs < 86_400.0 {
        format!(
            "{}h {:02}m",
            (secs / 3_600.0) as u64,
            (secs / 60.0) as u64 % 60
        )
    } else {
        format!(
            "{}d {}h",
            (secs / 86_400.0) as u64,
            (secs / 3_600.0) as u64 % 24
        )
    }
}

/// Compact age for "updated just now" style labels.
pub fn age_label(ms_ago: i64) -> String {
    if ms_ago < 45_000 {
        return tr!("usage.just_now");
    }
    if ms_ago < 60_000 * 60 {
        return tr!("usage.m_ago", count = ms_ago / 60_000);
    }
    if ms_ago < 86_400_000 {
        return tr!("usage.h_ago", count = ms_ago / 3_600_000);
    }
    tr!("usage.d_ago", count = ms_ago / 86_400_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_keeps_three_significant_digits() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(842), "842");
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1_000), "1K");
        assert_eq!(compact(1_250), "1.25K");
        assert_eq!(compact(12_400), "12.4K");
        assert_eq!(compact(842_100), "842K");
        assert_eq!(compact(1_284_000), "1.28M");
        assert_eq!(compact(18_421_842), "18.4M");
        assert_eq!(compact(2_100_000_000), "2.1B");
    }

    #[test]
    fn exact_groups_thousands() {
        assert_eq!(exact(0), "0");
        assert_eq!(exact(842), "842");
        assert_eq!(exact(1_000), "1,000");
        assert_eq!(exact(18_421_842), "18,421,842");
        assert_eq!(exact(123_456_789), "123,456,789");
    }

    #[test]
    fn costs_scale_from_cents_to_thousands() {
        assert_eq!(cost(0.0), "$0.00");
        assert_eq!(cost(0.004), "<$0.01");
        assert_eq!(cost(0.84), "$0.84");
        assert_eq!(cost(12.4), "$12.40");
        assert_eq!(cost(1_284.204), "$1,284.20");
    }

    #[test]
    fn durations_read_at_their_own_scale() {
        assert_eq!(duration_ms(420.0), "420ms");
        assert_eq!(duration_ms(1_800.0), "1.8s");
        assert_eq!(duration_ms(42_000.0), "42s");
        assert_eq!(duration_ms(134_000.0), "2m 14s");
        assert_eq!(duration_ms(8_040_000.0), "2h 14m");
        assert_eq!(span_ms(522_000), "8m 42s");
        assert_eq!(span_ms(7_200_000), "2h 00m");
    }

    #[test]
    fn percentages_do_not_round_away_small_numbers() {
        assert_eq!(percent(68.4), "68.4%");
        assert_eq!(percent(8.2), "8.2%");
        assert_eq!(percent(0.42), "0.42%");
        assert_eq!(percent(0.4), "0.4%");
        assert_eq!(percent(0.004), "<0.01%");
        assert_eq!(percent(52.0), "52%");
        assert_eq!(percent(100.0), "100%");
        // A sub-100 share never rounds up to a claimed whole.
        assert_eq!(percent(99.96), "99.9%");
        assert_eq!(percent(99.999), "99.9%");
        assert_eq!(share(0.52), "52%");
        assert_eq!(delta(Some(12.4)), "+12.4%");
        assert_eq!(delta(Some(-8.2)), "-8.2%");
        assert_eq!(delta(Some(0.01)), "no change");
        assert_eq!(delta(None), "no change");
    }
}
