//! What the core fetched for a config never reaches a page. A config can
//! point a `proxy-provider` or `rule-provider` anywhere, this machine's own
//! services included; the core then quotes what it read in its log (every
//! rule-set line it cannot parse, a YAML error's few characters, an HTTP
//! status line) and in the errors of updating it. A page gets the provider's
//! name and the kind of error, never those words. The core's own log file
//! (`core.log`) keeps them all, for the owner.
//!
//! The lines are mihomo 1.19.33's (`rules/provider/*_strategy.go`,
//! `component/resource/fetcher.go`, `hub/executor/executor.go`,
//! `adapter/provider/subscription_info.go`).

/// `payload` (a line of the core's log) as a page may see it: `None` when it
/// quotes nothing fetched and goes as it is.
pub fn log_line(payload: &str) -> Option<String> {
    const LINE_NOT_SHOWN: &str = "(Kairo does not show the line)";
    if payload.starts_with("skip invalid domain from rule provider:") {
        return Some(format!("A rule set skipped a line that is not a domain {LINE_NOT_SHOWN}"));
    }
    if payload.starts_with("parse classical rule [") {
        return Some(format!("A rule set skipped a line that is not a rule {LINE_NOT_SHOWN}"));
    }
    if payload.starts_with("invalid Ipcidr:") {
        return Some(format!("A rule set skipped a line that is not an IP range {LINE_NOT_SHOWN}"));
    }
    if payload.starts_with("[Provider] get subscription-userinfo:") {
        return Some("[Provider] get subscription-userinfo: the header could not be read".to_owned());
    }
    for prefix in ["initial proxy provider ", "initial rule provider "] {
        if let Some(rest) = payload.strip_prefix(prefix)
            && let Some((name, err)) = rest.split_once(" error: ")
        {
            return Some(format!("{prefix}{name} error: {}", error_kind(err)));
        }
    }
    if let Some(rest) = payload.strip_prefix("[Provider] ") {
        for middle in [" pull error: ", " read bundle file error: "] {
            if let Some((name, err)) = rest.split_once(middle) {
                return Some(format!("[Provider] {name}{middle}{}", error_kind(err)));
            }
        }
    }
    None
}

/// The error of updating provider `name`, as a page may see it.
pub fn update_error(name: &str, err: &str) -> String {
    format!("{name} was not updated: {}", error_kind(err))
}

/// What kind of error `err` is, in words of Kairo's own: nothing of `err`
/// itself goes through.
pub fn error_kind(err: &str) -> String {
    let e = err.trim();
    let code: String = e.chars().take(3).collect();
    if code.len() == 3 && code.chars().all(|c| c.is_ascii_digit()) && e.chars().nth(3).is_none_or(|c| c == ' ') {
        return format!("the server answered HTTP {code}");
    }
    let kinds: &[(&[&str], &str)] = &[
        (&["file must have a `payload` field"], "it has no `payload` field"),
        (&["file must have a `proxies` field"], "it has no `proxies` field"),
        (&["file doesn't have any proxy"], "it has no proxy"),
        (&["doesn't match any proxy"], "no proxy in it matches the filter"),
        (&["MrsMagic", "invalid behavior", "length is invalid"], "it is not a valid MRS rule set"),
        (&["invalid format"], "it is not in the format the config gives"),
        (&["yaml:"], "its content is not valid YAML"),
        (&["connection refused"], "nothing answered (connection refused)"),
        (&["no such host"], "its name does not resolve"),
        (&["timeout", "deadline exceeded"], "it did not answer in time"),
        (&["x509", "tls:", "certificate"], "its TLS certificate was not accepted"),
        (&["EOF", "connection reset"], "the connection closed early"),
    ];
    for (needles, kind) in kinds {
        if needles.iter().any(|n| e.contains(n)) {
            return (*kind).to_owned();
        }
    }
    if e.starts_with("proxy ") && e.contains(" error") {
        return "a proxy in it is not valid".to_owned();
    }
    "it could not be read".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "PROVIDER-SECRET-4242";

    #[test]
    fn the_lines_that_quote_what_was_fetched_say_only_what_kind_of_error() {
        let lines = [
            format!("skip invalid domain from rule provider: invalid domain \"<html><body>Internal dashboard: {SECRET} key=abc</body></html>\""),
            format!("skip invalid domain from rule provider: invalid domain \"a/{SECRET}\": slash is not allowed"),
            format!("parse classical rule [LINE-ONE-{SECRET} is here] error: unsupported rule type"),
            format!("invalid Ipcidr:[{SECRET}]"),
            format!("[Provider] get subscription-userinfo: failed to parse value '{SECRET}'"),
            format!("initial proxy provider extra error: yaml: unmarshal errors:\n  line 1: cannot unmarshal !!str `{}...` into", &SECRET[..7]),
            format!("initial rule provider local error: 418 {SECRET}"),
            format!("[Provider] local pull error: Get \"http://127.0.0.1:8080/{SECRET}\": EOF"),
            format!("[Provider] local read bundle file error: zip: {SECRET}"),
        ];
        for line in &lines {
            let shown = log_line(line).unwrap_or_else(|| panic!("{line} went as it is"));
            assert!(!shown.contains(&SECRET[..7]), "{shown}");
        }
        assert_eq!(log_line(&lines[5]).unwrap(), "initial proxy provider extra error: its content is not valid YAML");
        assert_eq!(log_line(&lines[6]).unwrap(), "initial rule provider local error: the server answered HTTP 418");
        assert_eq!(log_line(&lines[7]).unwrap(), "[Provider] local pull error: the connection closed early");
        // What quotes nothing fetched goes as it is.
        for line in ["[Provider] local's content update", "Start initial provider local", "[TCP] 127.0.0.1:1 --> a.com:443 match Match using DIRECT"] {
            assert_eq!(log_line(line), None);
        }
        assert_eq!(update_error("local", &format!("Get \"http://10.0.0.5/{SECRET}\": dial tcp 10.0.0.5:80: connect: connection refused")), "local was not updated: nothing answered (connection refused)");
        assert_eq!(error_kind(&format!("something new: {SECRET}")), "it could not be read");
    }
}
