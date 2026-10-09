//! The IANA time-zone database (compiled in through chrono-tz), in the shape tish-macos's
//! `macos.timeZones` has: zone ids, the local zone, a zone's offset and abbreviation at a moment,
//! and a zone for an abbreviation. Windows names its own zones differently ("Tokyo Standard
//! Time"), so the database ships with the app rather than coming from the system.

use chrono::{Offset, TimeZone};
use chrono_tz::{OffsetName, Tz, TZ_VARIANTS};

/// Every zone id, canonical ones only ("Asia/Tokyo"; no "Japan" or "Etc/GMT-9" aliases).
pub fn names() -> Vec<&'static str> {
    TZ_VARIANTS.iter().map(|z| z.name()).filter(|n| n.contains('/') && !n.starts_with("Etc/") && !n.starts_with("SystemV/")).collect()
}

/// The local zone's id, "UTC" when the system doesn't say.
pub fn local() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

/// (seconds east of UTC, abbreviation) for `id` at Unix time `unix`; None for an unknown id.
pub fn at(id: &str, unix: i64) -> Option<(i32, String)> {
    let tz: Tz = id.parse().ok()?;
    let t = tz.timestamp_opt(unix, 0).single()?;
    let off = t.offset();
    let abbr = off.abbreviation().map(str::to_string).unwrap_or_else(|| {
        // Zones without a letter abbreviation use "+09"-style names; build one from the offset.
        let s = off.fix().local_minus_utc();
        format!("{}{:02}", if s < 0 { '-' } else { '+' }, s.abs() / 3600)
    });
    Some((off.fix().local_minus_utc(), abbr))
}

/// Well-known abbreviations to the zone people mean by them (several zones share most).
const PREFERRED: &[(&str, &str)] = &[
    ("PST", "America/Los_Angeles"),
    ("PDT", "America/Los_Angeles"),
    ("MST", "America/Denver"),
    ("MDT", "America/Denver"),
    ("CST", "America/Chicago"),
    ("CDT", "America/Chicago"),
    ("EST", "America/New_York"),
    ("EDT", "America/New_York"),
    ("GMT", "Europe/London"),
    ("BST", "Europe/London"),
    ("CET", "Europe/Paris"),
    ("CEST", "Europe/Paris"),
    ("IST", "Asia/Kolkata"),
    ("JST", "Asia/Tokyo"),
    ("KST", "Asia/Seoul"),
    ("AEST", "Australia/Sydney"),
    ("AEDT", "Australia/Sydney"),
    ("UTC", "UTC"),
];

/// A zone using `abbr` now (or six months from now, for the other half of daylight saving).
pub fn by_abbreviation(abbr: &str) -> Option<String> {
    let a = abbr.to_ascii_uppercase();
    if let Some((_, z)) = PREFERRED.iter().find(|(k, _)| *k == a) {
        return Some(z.to_string());
    }
    let now = chrono::Utc::now().timestamp();
    for when in [now, now + 182 * 86_400] {
        for n in names() {
            if at(n, when).is_some_and(|(_, ab)| ab == a) {
                return Some(n.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_abbreviations() {
        // 2026-01-15 and 2026-07-15, noon UTC.
        let (jan, jul) = (1_768_478_400, 1_784_116_800);
        assert_eq!(at("Asia/Tokyo", jan), Some((9 * 3600, "JST".into())));
        assert_eq!(at("America/New_York", jan), Some((-5 * 3600, "EST".into())));
        assert_eq!(at("America/New_York", jul), Some((-4 * 3600, "EDT".into())));
        assert_eq!(at("Nowhere/Land", jan), None);
        assert!(names().contains(&"Europe/Paris"));
        assert!(!names().iter().any(|n| n.starts_with("Etc/")));
        assert_eq!(by_abbreviation("pst").as_deref(), Some("America/Los_Angeles"));
        assert!(by_abbreviation("NZST").is_some());
        assert!(!local().is_empty());
    }
}
