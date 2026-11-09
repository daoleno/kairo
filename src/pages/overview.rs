//! Dates in the user's words, for the pages that show a subscription.

pub fn expiry_text(expire: Option<i64>) -> String {
    match expire {
        None => "No expiry".into(),
        Some(t) => {
            let days = (t - kairo_core::profile::now()) / 86400;
            let date = date_of(t);
            if days < 0 {
                format!("Expired {date}")
            } else {
                format!("Expires {date} · {days} d left")
            }
        }
    }
}

/// `YYYY-MM-DD` of a unix time (local).
pub fn date_of(t: i64) -> String {
    let days = (t + kairo_core::service::local_offset_seconds()).div_euclid(86400);
    // Civil from days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}
