//! Minimal UTC civil-calendar rendering for `formatDateTime` — the only
//! date capability mxrb's native expression engine exposes. No external
//! date crate: the pattern subset is fixed (`yyyy MMM EEE MM dd HH mm ss`),
//! so days-from-epoch conversion (Howard Hinnant's algorithm) suffices.

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// 0 = Sunday.
    pub weekday: u32,
}

pub fn civil_from_epoch(seconds: f64) -> Civil {
    let total = seconds.floor() as i64;
    let days = total.div_euclid(86_400);
    let of_day = total.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Civil {
        year,
        month,
        day,
        hour: (of_day / 3600) as u32,
        minute: (of_day % 3600 / 60) as u32,
        second: (of_day % 60) as u32,
        weekday: (days + 4).rem_euclid(7) as u32,
    }
}

/// Days since 1970-01-01 → (year, month, day). Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Ports mxrb's `format_datetime` strftime translation table.
pub fn format(seconds: f64, pattern: &str) -> String {
    let civil = civil_from_epoch(seconds);
    let mut result = String::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        let replaced = [
            ("yyyy", format!("{:04}", civil.year)),
            ("MMM", MONTHS[(civil.month - 1) as usize].to_string()),
            ("EEE", WEEKDAYS[civil.weekday as usize].to_string()),
            ("MM", format!("{:02}", civil.month)),
            ("dd", format!("{:02}", civil.day)),
            ("HH", format!("{:02}", civil.hour)),
            ("mm", format!("{:02}", civil.minute)),
            ("ss", format!("{:02}", civil.second)),
        ]
        .into_iter()
        .find_map(|(token, rendering)| {
            let remaining = rest.strip_prefix(token)?;
            result.push_str(&rendering);
            Some(remaining)
        });
        match replaced {
            Some(remaining) => rest = remaining,
            None => {
                let mut characters = rest.chars();
                if let Some(character) = characters.next() {
                    result.push(character);
                }
                rest = characters.as_str();
            }
        }
    }
    result
}

pub fn to_string(seconds: f64) -> String {
    format(seconds, "yyyy-MM-dd HH:mm:ss") + " UTC"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_utc_instants_with_the_mxrb_pattern_table() {
        // 2026-09-21 is a Monday; 1758462896 = 2025-09-21T13:54:56Z.
        assert_eq!(format(0.0, "yyyy-MM-dd HH:mm:ss"), "1970-01-01 00:00:00");
        assert_eq!(format(0.0, "EEE MMM dd"), "Thu Jan 01");
        assert_eq!(format(951_827_696.0, "yyyy-MM-dd"), "2000-02-29");
        assert_eq!(format(951_827_696.0, "EEE"), "Tue");
        assert_eq!(format(-86_400.0, "yyyy-MM-dd EEE"), "1969-12-31 Wed");
        assert_eq!(format(0.0, "literal 'x' yyyy"), "literal 'x' 1970");
    }
}
