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

/// Where a line that may quote what was fetched starts.
const STARTS: &[&str] = &[
    "skip invalid domain from rule provider:",
    "parse classical rule [",
    "invalid Ipcidr:",
    "[Provider] ",
    "initial proxy provider ",
    "initial rule provider ",
];

/// `text` as a page may see it: from the first of the core's lines that
/// quotes what was fetched, the rest is said in Kairo's words. That line may
/// be anywhere: a log line is one, and a notice or a failure quotes the
/// core's last error line ("The core stopped unexpectedly and is restarting:
/// …"). `None` when it quotes nothing fetched and goes as it is.
///
/// `rule_sets` are the running config's rule sets (name, behavior): mihomo's
/// lines about a rule set's content name none, and rule sets load at the
/// same time, but each kind of line comes from one behavior.
pub fn in_text(text: &str, rule_sets: &[(String, String)]) -> Option<String> {
    let mut from = 0;
    loop {
        let start = STARTS.iter().filter_map(|s| text[from..].find(s).map(|i| from + i)).min()?;
        if let Some(said) = log_line(&text[start..], rule_sets) {
            return Some(format!("{}{said}", &text[..start]));
        }
        from = start + 1;
        while !text.is_char_boundary(from) {
            from += 1;
        }
    }
}

/// `payload`, starting with a line of the core's log, as a page may see it:
/// `None` when it quotes nothing fetched.
fn log_line(payload: &str, rule_sets: &[(String, String)]) -> Option<String> {
    let skipped = |behavior: &str, what: &str| {
        let names: Vec<&str> = rule_sets.iter().filter(|(_, b)| b == behavior).map(|(n, _)| n.as_str()).collect();
        let which = match names.as_slice() {
            [one] => format!("The rule set {one}"),
            [] => "A rule set".to_owned(),
            many => format!("A rule set (one of {})", many.join(", ")),
        };
        format!("{which} skipped a line that is not {what} (Kairo does not show the line)")
    };
    if payload.starts_with("skip invalid domain from rule provider:") {
        return Some(skipped("domain", "a domain"));
    }
    if payload.starts_with("parse classical rule [") {
        return Some(skipped("classical", "a rule"));
    }
    if payload.starts_with("invalid Ipcidr:") {
        return Some(skipped("ipcidr", "an IP range"));
    }
    if payload.starts_with("[Provider] get subscription-userinfo:") {
        return Some("[Provider] get subscription-userinfo: the header could not be read".to_owned());
    }
    for prefix in ["initial proxy provider ", "initial rule provider "] {
        if let Some(rest) = payload.strip_prefix(prefix)
            && let Some((name, err)) = rest.split_once(" error: ")
            && !name.contains('\n')
        {
            return Some(format!("{prefix}{name} error: {}", error_kind(err)));
        }
    }
    if let Some(rest) = payload.strip_prefix("[Provider] ") {
        for middle in [" pull error: ", " read bundle file error: "] {
            // A name is on one line: a later line's error is not this one's.
            if let Some((name, err)) = rest.split_once(middle)
                && !name.contains('\n')
            {
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
        let none: &[(String, String)] = &[];
        for line in &lines {
            let shown = in_text(line, none).unwrap_or_else(|| panic!("{line} went as it is"));
            assert!(!shown.contains(&SECRET[..7]), "{shown}");
        }
        assert_eq!(in_text(&lines[5], none).unwrap(), "initial proxy provider extra error: its content is not valid YAML");
        assert_eq!(in_text(&lines[6], none).unwrap(), "initial rule provider local error: the server answered HTTP 418");
        assert_eq!(in_text(&lines[7], none).unwrap(), "[Provider] local pull error: the connection closed early");
        // A rule set's lines name no rule set: the running config's of that behavior are named.
        let sets = [("dash", "domain"), ("cls", "classical"), ("more", "domain")].map(|(n, b)| (n.to_owned(), b.to_owned()));
        assert_eq!(
            in_text(&lines[2], &sets).unwrap(),
            "The rule set cls skipped a line that is not a rule (Kairo does not show the line)"
        );
        assert_eq!(
            in_text(&lines[0], &sets).unwrap(),
            "A rule set (one of dash, more) skipped a line that is not a domain (Kairo does not show the line)"
        );
        assert_eq!(
            in_text(&lines[3], &sets).unwrap(),
            "A rule set skipped a line that is not an IP range (Kairo does not show the line)"
        );
        // What quotes nothing fetched goes as it is.
        for line in ["[Provider] local's content update", "Start initial provider local", "[TCP] 127.0.0.1:1 --> a.com:443 match Match using DIRECT"] {
            assert_eq!(in_text(line, none), None);
        }
        // Anywhere in a text: a notice that quotes the core's last error line,
        // the way `explain` gives it (several lines, the first not the core's).
        let notice = format!(
            "The core stopped unexpectedly and is restarting: initial proxy provider pp error: yaml: unmarshal errors:\\n  line 1: cannot unmarshal !!str `LINE-ON...` into provider.ProxySchema\nparse classical rule [{SECRET}] error"
        );
        assert_eq!(
            in_text(&notice, none).unwrap(),
            "The core stopped unexpectedly and is restarting: initial proxy provider pp error: its content is not valid YAML"
        );
        let later = format!("The core exited: [Provider] pp's content update\nskip invalid domain from rule provider: invalid domain \"{SECRET}\"");
        let shown = in_text(&later, none).unwrap();
        assert!(shown.starts_with("The core exited: [Provider] pp's content update\nA rule set skipped") && !shown.contains(SECRET), "{shown}");
        assert_eq!(in_text("The core exited with code 1", none), None);
        let across = format!("[Provider] a's content update\nskip invalid domain from rule provider: invalid domain \"{SECRET}\"\n[Provider] b pull error: EOF");
        let shown = in_text(&across, none).unwrap();
        assert!(!shown.contains(SECRET), "{shown}");
        assert_eq!(update_error("local", &format!("Get \"http://10.0.0.5/{SECRET}\": dial tcp 10.0.0.5:80: connect: connection refused")), "local was not updated: nothing answered (connection refused)");
        assert_eq!(error_kind(&format!("something new: {SECRET}")), "it could not be read");
    }
}
