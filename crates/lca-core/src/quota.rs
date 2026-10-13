//! Subscription quota rendering for the generic `/usage` (gh #241C,
//! pi-antigravity `formatUsageSummary`): providers whose usage is a
//! subscription picture rather than token counts carry normalized
//! groups in usage extras, and this renders plan plus bars plus
//! reset countdowns. Split from `registry.rs` under the workspace's
//! 1,200-line file ceiling.

use lca_protocol::Usage;

/// The standard usage shape the generic `/usage` prints (ADR-0012):
/// token counts, unless the provider carried a quota picture in
/// `extras` (gh #241C) — a subscription summary does not decompose
/// into input/output, so it renders as plan plus quota bars.
pub(crate) fn format_usage(usage: &Usage) -> String {
    if usage.extras.contains_key(lca_protocol::QUOTA_GROUPS_EXTRA) {
        let plan = usage
            .extras
            .get(lca_protocol::QUOTA_PLAN_EXTRA)
            .map(String::as_str);
        let error = usage
            .extras
            .get(lca_protocol::QUOTA_ERROR_EXTRA)
            .map(String::as_str);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or(0);
        if let Some(text) = format_quota_usage(
            plan,
            &usage.extras[lca_protocol::QUOTA_GROUPS_EXTRA],
            error,
            now_ms,
        ) {
            return text;
        }
    }
    format!(
        "input {}, output {}, cache read {}, cache write {}, cost ${:.4}",
        usage.input, usage.output, usage.cache_read, usage.cache_write, usage.cost
    )
}

/// Render a quota picture (pi-antigravity `formatUsageSummary`): the
/// plan line, then per group a 20-wide bar plus `% left · resets`
/// countdown per bucket. `groups_json` is the normalized array the
/// provider wrote (`[{name, buckets: [{name, remaining 0-1,
/// reset}]}]`); unparseable input is `None` so the caller falls back
/// to token counts instead of showing garbage.
pub(crate) fn format_quota_usage(
    plan: Option<&str>,
    groups_json: &str,
    quota_error: Option<&str>,
    now_ms: u64,
) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Bucket {
        name: String,
        remaining: f64,
        reset: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Group {
        name: String,
        buckets: Vec<Bucket>,
    }
    let groups: Vec<Group> = serde_json::from_str(groups_json).ok()?;
    let mut lines = Vec::new();
    if let Some(plan) = plan {
        lines.push(plan.to_string());
    }
    if groups.is_empty() {
        lines.push(quota_error_note(quota_error));
        return Some(lines.join("\n"));
    }
    for group in &groups {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(group.name.clone());
        for bucket in &group.buckets {
            let remaining = bucket.remaining.clamp(0.0, 1.0);
            lines.push(format!(
                "  {} {}: {}% left · resets {}",
                progress_bar(remaining),
                bucket.name,
                remaining_percent(remaining),
                format_reset(bucket.reset.as_deref(), now_ms),
            ));
        }
    }
    Some(lines.join("\n").trim_end().to_string())
}

/// The empty-groups note: a paid-gate refusal says so with the
/// fallback, anything else reports the miss plainly.
fn quota_error_note(quota_error: Option<&str>) -> String {
    match quota_error {
        Some("subscription-required") => "Aggregate quota summary needs a paid subscription. Per-model usage is still available via /session.".to_string(),
        Some(other) => format!("Aggregate quota summary unavailable: {other}"),
        None => "No quota groups returned.".to_string(),
    }
}

/// A 20-wide bar, `#` filled by remaining share.
fn progress_bar(remaining: f64) -> String {
    let filled = (remaining * 20.0).round() as usize;
    let filled = filled.min(20);
    format!("[{}]", "#".repeat(filled) + &"-".repeat(20 - filled))
}

/// Tenths when fractional, bare integer when whole (`80`, `87.5`).
fn remaining_percent(remaining: f64) -> String {
    let tenths = (remaining * 1000.0).round() / 10.0;
    if tenths == tenths.trunc() {
        format!("{}", tenths as i64)
    } else {
        format!("{tenths}")
    }
}

/// A reset countdown against now (`2h 15m`), `now` past due, `n/a`
/// when absent; unparseable input echoes back (never a panic on
/// server text).
fn format_reset(reset: Option<&str>, now_ms: u64) -> String {
    let Some(text) = reset else {
        return "n/a".to_string();
    };
    let Some(reset_ms) = parse_rfc3339_ms(text) else {
        return text.to_string();
    };
    if reset_ms <= now_ms {
        return "now".to_string();
    }
    let total_min = (reset_ms - now_ms + 30_000) / 60_000;
    let days = total_min / (60 * 24);
    let hours = (total_min % (60 * 24)) / 60;
    let mins = total_min % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

/// Minimal RFC 3339 (`YYYY-MM-DDTHH:MM:SSZ`, optional fraction and
/// numeric offset): epoch millis, or `None` when the shape is foreign.
/// No date crate for one subtraction — the month table plus the
/// civil-date shift is the whole algorithm.
fn parse_rfc3339_ms(text: &str) -> Option<u64> {
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, zone) = if let Some(plus) = time.find('+') {
        time.split_at(plus)
    } else if let Some(minus) = time[1..].find('-').map(|at| at + 1) {
        time.split_at(minus)
    } else {
        (time.trim_end_matches('Z'), "+00:00")
    };
    let mut clock = clock.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: f64 = clock.next()?.parse().ok()?;
    if hour > 23 || minute > 59 || second >= 61.0 {
        return None;
    }
    let offset_min: i64 = if zone == "+00:00" || zone.eq_ignore_ascii_case("Z") {
        0
    } else {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let digits: String = zone.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() < 4 {
            return None;
        }
        let hours: i64 = digits[..2].parse().ok()?;
        let minutes: i64 = digits[2..4].parse().ok()?;
        sign * (hours * 60 + minutes)
    };
    // Days from civil date (Howard Hinnant's algorithm).
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146097 + day_of_era - 719468;
    let millis = ((days * 24 + hour) * 60 + minute) * 60 * 1000 + (second * 1000.0) as i64
        - offset_min * 60 * 1000;
    u64::try_from(millis).ok()
}

#[cfg(test)]
mod usage_tests {
    use super::{format_quota_usage, format_reset, progress_bar, remaining_percent};

    const GROUPS: &str = r#"[{"name":"Quota group","buckets":[{"name":"Limit","remaining":0.8,"reset":"2030-01-01T02:15:00Z"}]}]"#;

    // Verifies: gh #241C - the quota picture renders plan, bar,
    // percent, and countdown in pi-antigravity's exact shape.
    #[test]
    fn quota_picture_matches_pi_antigravity() {
        // 2030-01-01T00:00Z in epoch millis.
        let now_ms = 1893456000000;
        let text = format_quota_usage(Some("Google AI Pro (g1-pro-tier)"), GROUPS, None, now_ms)
            .expect("renders");
        assert_eq!(
            text,
            "Google AI Pro (g1-pro-tier)\n\nQuota group\n  [################----] Limit: 80% left · resets 2h 15m"
        );
    }

    // Verifies: gh #241C - empty groups degrade with the paid-gate
    // note, the generic note, or plain absence, never garbage.
    #[test]
    fn empty_groups_degrade_plainly() {
        let text = format_quota_usage(Some("Plan"), "[]", Some("subscription-required"), 0)
            .expect("renders");
        assert!(text.contains("paid subscription"), "{text}");
        let text = format_quota_usage(None, "[]", None, 0).expect("renders");
        assert_eq!(text, "No quota groups returned.");
        assert!(format_quota_usage(None, "not json", None, 0).is_none());
    }

    // Verifies: gh #241C - bars, percents, and resets on their edges.
    #[test]
    fn bars_percents_and_resets() {
        assert_eq!(progress_bar(1.0), "[####################]");
        assert_eq!(progress_bar(0.0), "[--------------------]");
        assert_eq!(remaining_percent(0.875), "87.5");
        assert_eq!(remaining_percent(0.8), "80");
        assert_eq!(format_reset(None, 0), "n/a");
        assert_eq!(
            format_reset(Some("2030-01-01T00:00:00Z"), 1893456000000),
            "now"
        );
        assert_eq!(format_reset(Some("bogs"), 0), "bogs");
    }
}
