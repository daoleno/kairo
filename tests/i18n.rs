//! The catalog (`locales/`) and the code that reads it:
//!
//! - every key has English and Simplified Chinese, with the same `%{name}`s,
//!   and lives in one file only;
//! - every key the code names exists, and every key is named somewhere;
//! - no text a person reads is written in the code instead of the catalog
//!   (string literals that read as English, outside `t!`/`tr!`, logs, tests
//!   and the exceptions `tests/i18n-allow.txt` lists with their reasons).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const LANGUAGES: [&str; 2] = ["en", "zh"];

/// The code whose literals are checked: the window, the tray and the engine
/// (whose messages reach the window and notifications).
const CODE: [&str; 3] = ["src", "crates/kairo-tray/src", "crates/kairo-core/src"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn files(dir: &Path, ext: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files(&path, ext, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ext.contains(&e))
        {
            out.push(path);
        }
    }
}

/// key → language → text, and the file each key is in.
struct Catalog {
    texts: BTreeMap<String, BTreeMap<String, String>>,
    problems: Vec<String>,
}

fn catalog() -> Catalog {
    let mut paths = Vec::new();
    files(&root().join("locales"), &["yml", "yaml", "json", "toml"], &mut paths);
    assert!(!paths.is_empty(), "no files in locales/");
    let mut texts: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut home: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut problems = Vec::new();
    for path in paths {
        let name = path.strip_prefix(root()).unwrap().display().to_string();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
        assert!(matches!(ext, "yml" | "yaml"), "{name}: the catalog is YAML");
        let source = fs::read_to_string(&path).unwrap();
        let doc: serde_yaml::Mapping = serde_yaml::from_str(&source)
            .unwrap_or_else(|e| panic!("{name}: {e} (a key twice in one file?)"));
        let version = doc.get("_version").and_then(|v| v.as_u64());
        if version != Some(2) {
            problems.push(format!("{name}: `_version: 2` (each key with its languages)"));
            continue;
        }
        for (key, value) in doc {
            let key = key.as_str().unwrap_or_default().to_owned();
            if key == "_version" {
                continue;
            }
            if let Some(first) = home.insert(key.clone(), path.clone()) {
                problems.push(format!("{key}: in {} and {name}", first.display()));
            }
            let Some(map) = value.as_mapping() else {
                problems.push(format!("{name}: {key} is not a map of languages"));
                continue;
            };
            let mut by_language = BTreeMap::new();
            for (language, text) in map {
                let language = language.as_str().unwrap_or_default().to_owned();
                match text.as_str() {
                    Some(t) => {
                        by_language.insert(language, t.to_owned());
                    }
                    None => problems.push(format!("{name}: {key}.{language} is not text")),
                }
            }
            texts.insert(key, by_language);
        }
    }
    Catalog { texts, problems }
}

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("%{") {
        let after = &rest[i + 2..];
        match after.find('}') {
            Some(j) => {
                out.insert(after[..j].to_owned());
                rest = &after[j + 1..];
            }
            None => break,
        }
    }
    out
}

#[test]
fn every_key_has_english_and_chinese_with_the_same_arguments() {
    let catalog = catalog();
    let mut problems = catalog.problems;
    for (key, texts) in &catalog.texts {
        for language in LANGUAGES {
            match texts.get(language) {
                Some(t) if !t.trim().is_empty() => {}
                _ => problems.push(format!("{key}: no {language}")),
            }
        }
        for language in texts.keys() {
            if !LANGUAGES.contains(&language.as_str()) {
                problems.push(format!("{key}: a language Kairo does not ship: {language}"));
            }
        }
        if let (Some(en), Some(zh)) = (texts.get("en"), texts.get("zh")) {
            if placeholders(en) != placeholders(zh) {
                problems.push(format!("{key}: %{{…}} differ: {en:?} / {zh:?}"));
            }
        }
    }
    assert!(problems.is_empty(), "the catalog:\n  {}", problems.join("\n  "));
}

#[test]
fn the_engine_reads_the_same_catalog() {
    let ours = catalog().texts;
    let engine = kairo_core::i18n::catalog();
    for language in LANGUAGES {
        let theirs = engine.get(language).cloned().unwrap_or_default();
        for (key, texts) in &ours {
            assert_eq!(theirs.get(key), texts.get(language), "{language}.{key}");
        }
    }
}

// --- The code -------------------------------------------------------------------------

/// A string literal of the code: its text, line, and what stands before it in
/// its statement (from the last `;`, `{` or `}` outside strings).
struct Literal {
    file: String,
    line: usize,
    text: String,
    before: String,
}

/// The string literals of `source` outside comments and `#[cfg(test)]` items.
fn literals(file: &str, source: &str) -> Vec<Literal> {
    let src: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    // Depth of braces, and the depth at which a skipped test item started.
    let mut depth = 0usize;
    let mut skip_from: Option<usize> = None;
    let mut test_attr = false;
    // Where the statement the next literal is in starts.
    let mut stmt = 0;
    while i < src.len() {
        let c = src[i];
        let next = src.get(i + 1).copied();
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c == '/' && next == Some('/') {
            while i < src.len() && src[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && next == Some('*') {
            let mut nest = 0;
            while i < src.len() {
                if src[i] == '/' && src.get(i + 1) == Some(&'*') {
                    nest += 1;
                    i += 2;
                } else if src[i] == '*' && src.get(i + 1) == Some(&'/') {
                    nest -= 1;
                    i += 2;
                    if nest == 0 {
                        break;
                    }
                } else {
                    if src[i] == '\n' {
                        line += 1;
                                }
                    i += 1;
                }
            }
            continue;
        }
        if c == '#' && src[i..].iter().take(12).collect::<String>() == "#[cfg(test)]" {
            test_attr = true;
            i += 12;
            continue;
        }
        if c == '{' {
            stmt = i + 1;
            depth += 1;
            if test_attr && skip_from.is_none() {
                skip_from = Some(depth);
            }
            test_attr = false;
            i += 1;
            continue;
        }
        if c == '}' {
            stmt = i + 1;
            if skip_from == Some(depth) {
                skip_from = None;
            }
            depth = depth.saturating_sub(1);
            i += 1;
            continue;
        }
        if c == ';' {
            stmt = i + 1;
        }
        if c == ';' && skip_from.is_none() {
            // `#[cfg(test)] mod tests;` or `#[cfg(test)] use …;`
            test_attr = false;
        }
        // A char literal ('"', '\'') or a lifetime ('a).
        if c == '\'' {
            if next == Some('\\') {
                let mut j = i + 2;
                while j < src.len() && src[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            if src.get(i + 2) == Some(&'\'') {
                i += 3;
                continue;
            }
            i += 1;
            continue;
        }
        // Raw strings: r"…", r#"…"#, br"…".
        let raw = (c == 'r' && matches!(next, Some('"') | Some('#')))
            || (c == 'b' && next == Some('r'));
        let ident_before = i > 0 && (src[i - 1].is_alphanumeric() || src[i - 1] == '_');
        if raw && !ident_before {
            let mut j = i + if c == 'b' { 2 } else { 1 };
            let mut hashes = 0;
            while src.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if src.get(j) == Some(&'"') {
                let start = j + 1;
                let mut k = start;
                let close: String = std::iter::once('"').chain("#".repeat(hashes).chars()).collect();
                while k < src.len() && src[k..].iter().take(close.len()).collect::<String>() != close {
                    k += 1;
                }
                let text: String = src[start..k.min(src.len())].iter().collect();
                if skip_from.is_none() {
                    out.push(Literal {
                        file: file.into(),
                        line,
                        text: text.clone(),
                        before: src[stmt.min(i)..i].iter().collect(),
                    });
                }
                line += text.matches('\n').count();
                i = k + close.len();
                continue;
            }
        }
        if c == '"' {
            let start = i;
            let mut j = i + 1;
            let mut text = String::new();
            while j < src.len() && src[j] != '"' {
                if src[j] == '\\' {
                    match src.get(j + 1) {
                        Some('n') => text.push('\n'),
                        Some('\n') => {
                            // A continued line: its leading space is not in the text.
                            j += 2;
                            line += 1;
                            while j < src.len() && src[j].is_whitespace() {
                                if src[j] == '\n' {
                                    line += 1;
                                }
                                j += 1;
                            }
                            continue;
                        }
                        Some(other) => text.push(*other),
                        None => {}
                    }
                    j += 2;
                    continue;
                }
                text.push(src[j]);
                j += 1;
            }
            if skip_from.is_none() {
                out.push(Literal {
                    file: file.into(),
                    line,
                    text: text.clone(),
                    before: src[stmt.min(start)..start].iter().collect(),
                });
            }
            line += text.matches('\n').count();
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

fn code_literals() -> Vec<Literal> {
    let mut out = Vec::new();
    for dir in CODE {
        let mut paths = Vec::new();
        files(&root().join(dir), &["rs"], &mut paths);
        for path in paths {
            let name = path.strip_prefix(root()).unwrap().display().to_string();
            out.extend(literals(&name, &fs::read_to_string(&path).unwrap()));
        }
    }
    out
}

/// The key a literal names when it is the first argument of `t!`, `tr!` or
/// `plural(` (`plural`: both of its forms).
fn named_keys(lit: &Literal) -> Vec<String> {
    let b = lit.before.trim_end();
    let call = |name: &str| {
        b.strip_suffix(name)
            .is_some_and(|rest| !rest.ends_with(|c: char| c.is_alphanumeric() || c == '_'))
    };
    if call("t!(") || call("tr!(") {
        vec![lit.text.clone()]
    } else if call("plural(") {
        vec![format!("{}.one", lit.text), format!("{}.other", lit.text)]
    } else {
        vec![]
    }
}

#[test]
fn every_key_the_code_names_exists_and_every_key_is_named() {
    let catalog = catalog().texts;
    let literals = code_literals();
    let mut named = BTreeSet::new();
    let mut problems = Vec::new();
    for lit in &literals {
        for key in named_keys(lit) {
            if !catalog.contains_key(&key) {
                problems.push(format!("{}:{}: no key {key}", lit.file, lit.line));
            }
            named.insert(key);
        }
    }
    // A key may also be named as a literal handed on (a table of keys): one
    // in a namespace of the catalog must be in it.
    let namespaces: BTreeSet<&str> = catalog.keys().filter_map(|k| k.split('.').next()).collect();
    let files = [".json", ".yml", ".yaml", ".toml", ".txt", ".log", ".svg", ".png", ".rs"];
    for lit in &literals {
        let t = lit.text.as_str();
        let key_like = t.contains('.')
            && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
            && !files.iter().any(|f| t.ends_with(f));
        // A toolbar tool's id (`settings.tab`) is no key.
        let b = lit.before.trim_end();
        let tool_id = ["toolbar::search(", "toolbar::choice(", "toolbar::button(", "toolbar::button_dyn("]
            .iter()
            .any(|c| b.ends_with(c));
        if key_like
            && !tool_id
            && namespaces.contains(t.split('.').next().unwrap())
            && !catalog.contains_key(t)
            && !(catalog.contains_key(&format!("{t}.one")) && catalog.contains_key(&format!("{t}.other")))
        {
            problems.push(format!("{}:{}: no key {t}", lit.file, lit.line));
        }
    }
    let mentioned: BTreeSet<&str> = literals.iter().map(|l| l.text.as_str()).collect();
    for key in catalog.keys() {
        let base = key
            .strip_suffix(".one")
            .or_else(|| key.strip_suffix(".other"))
            .unwrap_or(key);
        if !named.contains(key) && !mentioned.contains(key.as_str()) && !mentioned.contains(base) {
            problems.push(format!("{key}: in the catalog, named nowhere"));
        }
    }
    assert!(problems.is_empty(), "keys:\n  {}", problems.join("\n  "));
}

/// The literals that may stay in the code, with why (`tests/i18n-allow.txt`).
fn allowed() -> BTreeSet<String> {
    let text = fs::read_to_string(root().join("tests/i18n-allow.txt")).unwrap();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (literal, why) = l.split_once('\t').unwrap_or_else(|| {
                panic!("tests/i18n-allow.txt: `{l}` needs a tab and a reason")
            });
            assert!(!why.trim().is_empty(), "tests/i18n-allow.txt: `{literal}` needs a reason");
            literal.to_owned()
        })
        .collect()
}

/// Whether a literal reads as English a person would see: two words, or one
/// capitalized word ("Back", "Connected"). Identifiers, paths, URLs, keys,
/// format strings without words and all-capital tokens do not.
fn reads_as_text(text: &str) -> bool {
    // `{n}`, `{:.0}`, `%{name}`: arguments, not words.
    let mut plain = String::new();
    let mut inside = false;
    for c in text.chars() {
        match c {
            '{' => inside = true,
            '}' => {
                inside = false;
                plain.push('\u{1}');
            }
            _ if !inside => plain.push(c),
            _ => {}
        }
    }
    let argument = plain.contains('\u{1}');
    let text = plain.as_str();
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_ascii_alphabetic() || c == '\''))
        .filter(|w| w.chars().filter(char::is_ascii_alphabetic).count() >= 2)
        .collect();
    if words.is_empty() {
        return false;
    }
    // Markup (SVG) and URLs.
    if text.contains("://") || (text.contains('<') && text.contains('>')) {
        return false;
    }
    let spaced = text.contains(' ');
    let code_like = !spaced
        && (text.contains(['/', '_', '.', ':', '+', '=', '-', '@', '\\'])
            || text.chars().any(|c| c.is_ascii_digit()));
    if code_like {
        return false;
    }
    if words.len() >= 2 && spaced {
        return true;
    }
    // A word with an argument: "{n} servers" (not a unit: "{} ms").
    if argument && words.len() == 1 && words[0].len() >= 3 && words[0].chars().any(|c| c.is_ascii_lowercase()) {
        return true;
    }
    // One word: a capitalized English word (not CamelCase, not all capitals).
    let w = words[0];
    let mut chars = w.chars();
    let first = chars.next().unwrap();
    first.is_ascii_uppercase() && chars.all(|c| c.is_ascii_lowercase() || c == '\'')
}

/// What a literal stands in that makes it no text for a person: a log line,
/// a panic, a key, an attribute, a test's assertion.
fn exempt_context(before: &str) -> bool {
    const CALLS: [&str; 22] = [
        "t!(", "tr!(", "plural(", "log::", "debug!(", "info!(", "warn!(", "error!(", "trace!(",
        "panic!(", "unreachable!(", "todo!(", ".expect(", "assert!(", "assert_eq!(",
        "assert_ne!(", "debug_assert!(", "#[name", "env!(", "include_str!(", "include_bytes!(",
        "#[doc",
    ];
    // The call is a whole name (`t!(`, not `format!(`), in this statement.
    CALLS.iter().any(|call| {
        before.match_indices(call).any(|(i, _)| {
            let starts_name = call.starts_with(|c: char| c.is_alphanumeric());
            !starts_name
                || !before[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_')
        })
    })
}

#[test]
fn no_text_a_person_reads_is_written_in_the_code() {
    let allowed = allowed();
    let mut used = BTreeSet::new();
    let mut problems = Vec::new();
    for lit in code_literals() {
        if !reads_as_text(&lit.text) || exempt_context(&lit.before) {
            continue;
        }
        if allowed.contains(&lit.text) {
            used.insert(lit.text.clone());
            continue;
        }
        problems.push(format!("{}:{}: {:?}", lit.file, lit.line, lit.text));
    }
    for literal in &allowed {
        if !used.contains(literal) {
            problems.push(format!("tests/i18n-allow.txt: {literal:?} is in the code no more"));
        }
    }
    assert!(
        problems.is_empty(),
        "{} literals read as text; move them to locales/ (t! in the window, tr! outside it) \
         or list them in tests/i18n-allow.txt with why:\n  {}",
        problems.len(),
        problems.join("\n  ")
    );
}

#[test]
fn the_scanner_reads_rust() {
    let src = r##"
        fn f() {
            let a = t!("page.home"); // "Not text"
            let b = "Hello there";
            /* "Block comment" */
            let c = r#"Raw "quoted" text"#;
            let d = '"';
            log::warn!("Not shown");
            let e = "line \
                     continued";
        }
        #[cfg(test)]
        mod tests {
            fn g() { let x = "Inside a test"; }
        }
        fn h() { let y = "After the tests"; }
    "##;
    let found: Vec<String> = literals("x.rs", src).into_iter().map(|l| l.text).collect();
    assert_eq!(
        found,
        ["page.home", "Hello there", "Raw \"quoted\" text", "Not shown", "line continued", "After the tests"]
    );
    assert!(reads_as_text("Hello there"));
    assert!(reads_as_text("Back"));
    assert!(reads_as_text("{n} servers"));
    assert!(!reads_as_text("win.palette"));
    assert!(!reads_as_text("edit-copy-symbolic"));
    assert!(!reads_as_text("KairoSidebar"));
    assert!(!reads_as_text("TUN"));
    assert!(!reads_as_text("{} ms"));
    assert!(!reads_as_text("https://example.com/a b"));
    assert!(!reads_as_text("<path d=\"M4 17l5-5\"/>"));
}
