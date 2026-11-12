//! The bundled logos (`assets/logos`, each file's source and licence in `assets/logos/SOURCES.md`):
//! a small set of well-known apps, sites and AI services, found by what they are called.

use crate::logos_table::LOGOS;

/// What is being named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    /// A program: `firefox`, `Telegram`.
    App,
    /// A site: `github.com` (its subdomains too).
    Site,
    /// An AI service by its id: `claude`.
    Service,
}

/// The SVG of the bundled logo for `name`, if there is one. A site matches a domain or any
/// domain under it, the longest first (`githubcopilot.com` before `github.com`); a program or a
/// service matches a whole name, as the matching of apps reads it (`Firefox`, `chrome.exe`).
pub fn bundled(named: Named, name: &str) -> Option<&'static str> {
    let key = match named {
        Named::Site => name.trim().trim_end_matches('.').to_lowercase(),
        _ => kairo_core::icons::program_name(name),
    };
    if key.is_empty() {
        return None;
    }
    let mut best: Option<(usize, &'static str)> = None;
    for (_, svg, aliases) in LOGOS {
        for alias in *aliases {
            let hit = match named {
                Named::Site => {
                    alias.contains('.') && (key == *alias || key.ends_with(&format!(".{alias}")))
                }
                _ => !alias.contains('.') && key == *alias,
            };
            if hit && best.is_none_or(|(len, _)| alias.len() > len) {
                best = Some((alias.len(), svg));
            }
        }
    }
    best.map(|(_, svg)| svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_names_have_their_logo() {
        let has = |n, name: &str| bundled(n, name).is_some();
        for site in [
            "github.com", "gist.github.com", "youtube.com", "www.youtube.com", "netflix.com", "telegram.org",
            "x.com", "reddit.com", "wikipedia.org", "en.wikipedia.org", "openai.com", "claude.ai", "t.me",
        ] {
            assert!(has(Named::Site, site), "{site}");
        }
        for app in ["firefox", "Firefox", "chrome", "chrome.exe", "Telegram", "telegram-desktop", "zed", "obs", "ghostty", "bitwarden", "thunderbird", "discord"] {
            assert!(has(Named::App, app), "{app}");
        }
        for id in ["claude", "chatgpt", "gemini", "cursor", "copilot", "perplexity", "grok"] {
            assert!(has(Named::Service, id), "{id}");
        }
        // Not a logo: an unknown name, a program that is only a part of a known name, a domain that merely ends alike.
        assert!(!has(Named::App, "python3.14"));
        assert!(!has(Named::App, "git"));
        assert!(!has(Named::Site, "notgithub.com"));
        assert!(!has(Named::Site, "example.test"));
        assert!(!has(Named::Site, ""));
    }

    #[test]
    fn the_longest_name_wins() {
        let copilot = bundled(Named::Site, "githubcopilot.com").unwrap();
        let github = bundled(Named::Site, "github.com").unwrap();
        assert_ne!(copilot, github);
        assert_eq!(bundled(Named::Site, "api.github.com"), Some(github));
    }

    #[test]
    fn every_logo_is_a_clean_svg() {
        for (key, svg, aliases) in LOGOS {
            assert!(svg.starts_with("<svg") && svg.contains("viewBox"), "{key}");
            assert!(!svg.contains("<script") && !svg.contains("<image") && !svg.contains("style="), "{key}");
            assert!(!aliases.is_empty(), "{key}");
        }
    }
}
