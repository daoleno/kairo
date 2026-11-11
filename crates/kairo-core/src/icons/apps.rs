//! An installed app's own icon, from the system, by the program's name (what Activity and
//! "Apps & Sites" call it).
//!
//! - **Linux**: the `.desktop` file of the program (matched by its `Exec`, its file name, its
//!   `StartupWMClass` or its `Name`), its `Icon=` looked up in the icon themes (the desktop's own
//!   theme first, then `hicolor` and the rest) or a file path.
//! - **macOS**: the `.app` bundle in the usual folders (matched by its name, else by its
//!   `CFBundleExecutable`), its `.icns`, made a PNG by `sips`.
//! - **Windows**: the icon of the running program's executable (`SHGetFileInfo`).
//! - **Android**: the package's icon from `PackageManager`, through the Java side.
//!
//! The lookups block and read files: they run on a blocking thread, never on the UI's.

use super::{Icon, normalize, trusted};

/// What the system knows of an installed app: its icon and the name it shows people (the
/// program is `firefox`, the app is "Firefox").
#[derive(Debug, Clone, Default)]
pub struct Found {
    pub icon: Option<Icon>,
    pub label: Option<String>,
}

/// A display name made safe to show: one line, no control characters, at most 60 characters.
pub fn clean_label(text: &str) -> Option<String> {
    let one: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    let one: String = one.chars().take(60).collect();
    (!one.is_empty()).then_some(one)
}

/// The app called `name`, if the system has one: its icon and its display name.
pub fn lookup(name: &str) -> Option<Found> {
    let n = program_name(name);
    if n.is_empty() {
        return None;
    }
    #[cfg(target_os = "linux")]
    return linux::lookup(&n);
    #[cfg(target_os = "macos")]
    return mac::lookup(&n);
    #[cfg(windows)]
    return win::lookup(&n);
    #[cfg(target_os = "android")]
    return android_lookup(&n);
    #[allow(unreachable_code)]
    {
        let _ = (normalize, trusted);
        None
    }
}

/// `Firefox`, `firefox-bin`, `chrome.exe` → `firefox`, `firefox`, `chrome`: a program's name as
/// the matching reads it.
pub fn program_name(name: &str) -> String {
    let mut n = name.trim().to_lowercase();
    for suffix in [".exe", ".app", "-bin", ".bin", "-desktop"] {
        if let Some(s) = n.strip_suffix(suffix) {
            n = s.to_owned();
        }
    }
    n
}

#[cfg(target_os = "android")]
fn android_lookup(name: &str) -> Option<Found> {
    let icon = crate::android::app_icon(name).and_then(|png| normalize(&png).ok());
    let label = crate::android::app_label(name).and_then(|l| clean_label(&l));
    (icon.is_some() || label.is_some()).then_some(Found { icon, label })
}

// --- Linux -------------------------------------------------------------------------------

#[cfg(any(target_os = "linux", test))]
pub mod linux {
    use std::path::{Path, PathBuf};

    use super::{normalize, trusted};

    /// What a `.desktop` file says about a program.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct Entry {
        pub id: String,
        pub name: String,
        pub exec: String,
        pub icon: String,
        pub wm_class: String,
    }

    /// The `[Desktop Entry]` group of a `.desktop` file.
    pub fn parse(id: &str, text: &str) -> Option<Entry> {
        let mut e = Entry {
            id: id.to_owned(),
            ..Entry::default()
        };
        let mut in_entry = false;
        let mut app = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k.trim() {
                "Type" => app = v.trim() == "Application",
                "Name" => e.name = v.trim().to_owned(),
                "Exec" => e.exec = v.trim().to_owned(),
                "Icon" => e.icon = v.trim().to_owned(),
                "StartupWMClass" => e.wm_class = v.trim().to_owned(),
                _ => {}
            }
        }
        (app && !e.icon.is_empty()).then_some(e)
    }

    /// The program an `Exec` line starts: its file name, past `env VAR=1`, `flatpak run`, `sh -c`.
    pub fn exec_program(exec: &str) -> String {
        let mut words = exec.split_whitespace().peekable();
        let mut flatpak = false;
        while let Some(w) = words.next() {
            let w = w.trim_matches('"');
            let base = w.rsplit('/').next().unwrap_or(w);
            if base == "env" || w.contains('=') && !w.starts_with('/') {
                continue;
            }
            if base == "flatpak" {
                flatpak = true;
                continue;
            }
            if flatpak && (w == "run" || w.starts_with("--")) {
                continue;
            }
            if matches!(base, "sh" | "bash" | "dash") {
                continue;
            }
            if w.starts_with('-') && !flatpak {
                continue;
            }
            return base.to_lowercase();
        }
        String::new()
    }

    /// A browser's web app ("Linear" in Chrome): its entry runs the browser's own binary with
    /// `--app-id=`/`--app=`, is named `chrome-<32 letters>-<profile>` (or `msedge-`, `brave-` …) and
    /// has the window class `crx_<id>`. It is an app of its own and never the browser.
    pub fn is_web_app(e: &Entry) -> bool {
        let id = e.id.to_lowercase();
        let named = id.split_once('-').is_some_and(|(browser, rest)| {
            matches!(
                browser,
                "chrome" | "msedge" | "brave" | "chromium" | "vivaldi" | "opera" | "google"
            ) && {
                // `google-chrome-<id>-Default` has one more word.
                let rest = rest.strip_prefix("chrome-").unwrap_or(rest);
                rest.split('-').next().is_some_and(|w| {
                    w.len() == 32 && w.bytes().all(|b| (b'a'..=b'p').contains(&b))
                })
            }
        });
        named
            || e.wm_class.to_lowercase().starts_with("crx_")
            || e.exec
                .split_whitespace()
                .any(|w| w.starts_with("--app-id=") || w.starts_with("--app="))
    }

    /// How well a desktop file stands for the program `n` (0: not at all).
    pub fn score(e: &Entry, n: &str) -> u32 {
        let id = e.id.trim_end_matches(".desktop").to_lowercase();
        let last = id.rsplit('.').next().unwrap_or(&id).to_owned();
        // A web app runs the browser's binary, so its `Exec` says nothing about which program
        // it is: only its own class, id and name count.
        let web = is_web_app(e);
        let exec = if web { String::new() } else { exec_program(&e.exec) };
        if !exec.is_empty() && exec == n {
            return 100;
        }
        if id == n || last == n || e.wm_class.to_lowercase() == n {
            return 90;
        }
        if e.name.to_lowercase() == n {
            return 80;
        }
        let tokens = |s: &str| -> Vec<String> {
            s.split(['-', '_', '.', ' '])
                .filter(|t| !t.is_empty())
                .map(str::to_owned)
                .collect()
        };
        if n.len() >= 4 && !web && (tokens(&exec).iter().any(|t| t == n) || tokens(&last).iter().any(|t| t == n)) {
            return 60;
        }
        0
    }

    /// The directories `.desktop` files live in.
    pub fn app_dirs() -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        let mut out: Vec<PathBuf> = Vec::new();
        out.extend(data_home.iter().map(|d| d.join("applications")));
        out.extend(
            data_dirs
                .split(':')
                .filter(|d| !d.is_empty())
                .map(|d| Path::new(d).join("applications")),
        );
        out.push("/var/lib/flatpak/exports/share/applications".into());
        if let Some(h) = &home {
            out.push(h.join(".local/share/flatpak/exports/share/applications"));
        }
        out.push("/var/lib/snapd/desktop/applications".into());
        out
    }

    /// The best desktop file for `n` in `dirs` (each searched one level down too).
    pub fn find(dirs: &[PathBuf], n: &str) -> Option<Entry> {
        let mut best: Option<(u32, Entry)> = None;
        let mut consider = |path: &Path| {
            if path.extension().is_none_or(|e| e != "desktop") {
                return;
            }
            let Ok(text) = std::fs::read_to_string(path) else {
                return;
            };
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(e) = parse(&id, &text) {
                let s = score(&e, n);
                if s > 0 && best.as_ref().is_none_or(|(b, _)| s > *b) {
                    best = Some((s, e));
                }
            }
        };
        for dir in dirs {
            let Ok(read) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in read.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    if let Ok(sub) = std::fs::read_dir(&p) {
                        sub.flatten().for_each(|e| consider(&e.path()));
                    }
                } else {
                    consider(&p);
                }
            }
        }
        best.map(|(_, e)| e)
    }

    /// The directories icon themes live in.
    pub fn icon_roots() -> Vec<PathBuf> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut out = Vec::new();
        if let Some(h) = &home {
            out.push(h.join(".icons"));
            out.push(h.join(".local/share/icons"));
        }
        if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
            out.push(Path::new(&d).join("icons"));
        }
        let data_dirs = std::env::var("XDG_DATA_DIRS")
            .unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
        out.extend(data_dirs.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("icons")));
        out.push("/usr/share/icons".into());
        out
    }

    /// The icon theme the desktop uses, from GTK's settings, when it says.
    pub fn current_theme() -> Option<String> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        for file in ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"] {
            let Ok(text) = std::fs::read_to_string(home.join(".config").join(file)) else {
                continue;
            };
            for line in text.lines() {
                if let Some(v) = line.trim().strip_prefix("gtk-icon-theme-name") {
                    let v = v.trim_start().trim_start_matches('=').trim();
                    if !v.is_empty() {
                        return Some(v.to_owned());
                    }
                }
            }
        }
        None
    }

    /// The side a directory name stands for: `64x64`, `64`, `48x48@2`; 0 for `scalable`.
    fn side(dir: &str) -> u32 {
        dir.split(['x', '@'])
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// Where `icon` is in one theme folder, best first: a PNG of 64 to 256 points (nearest to
    /// 128), else a scalable SVG, else any PNG. Both layouts are read: `64x64/apps/` and
    /// `apps/64/`.
    pub fn in_theme(theme: &Path, icon: &str) -> Option<PathBuf> {
        let mut png: Vec<(u32, PathBuf)> = Vec::new();
        let mut svg: Option<PathBuf> = None;
        let subdirs = |p: &Path| -> Vec<(String, PathBuf)> {
            std::fs::read_dir(p)
                .map(|r| {
                    r.flatten()
                        .filter(|e| e.path().is_dir())
                        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut look = |dir: &Path, size: u32| {
            let p = dir.join(format!("{icon}.png"));
            if p.is_file() {
                png.push((size, p));
            }
            let s = dir.join(format!("{icon}.svg"));
            if s.is_file() && svg.is_none() {
                svg = Some(s);
            }
        };
        for (name, path) in subdirs(theme) {
            if name == "apps" {
                // apps/<size>/
                for (sz, p) in subdirs(&path) {
                    look(&p, side(&sz));
                }
                look(&path, 0);
            } else {
                // <size>/apps/ and scalable/apps/
                let apps = path.join("apps");
                if apps.is_dir() {
                    let s = if name == "scalable" { 0 } else { side(&name) };
                    look(&apps, s);
                }
            }
        }
        png.retain(|(s, _)| *s > 0);
        let best = png
            .iter()
            .filter(|(s, _)| (64..=256).contains(s))
            .min_by_key(|(s, _)| s.abs_diff(128))
            .cloned();
        if let Some((_, p)) = best {
            return Some(p);
        }
        if let Some(s) = svg {
            return Some(s);
        }
        png.into_iter().max_by_key(|(s, _)| *s).map(|(_, p)| p)
    }

    /// The file an `Icon=` value names: itself when it is a path, else the best in the themes
    /// (the desktop's own first, then `hicolor`, then the others), else `pixmaps`.
    pub fn find_icon(icon: &str, roots: &[PathBuf], current: Option<&str>) -> Option<PathBuf> {
        if icon.starts_with('/') || Path::new(icon).is_absolute() {
            return Path::new(icon).is_file().then(|| PathBuf::from(icon));
        }
        let mut themes: Vec<PathBuf> = Vec::new();
        let mut push = |name: &str| {
            for r in roots {
                let t = r.join(name);
                if t.is_dir() && !themes.contains(&t) {
                    themes.push(t);
                }
            }
        };
        if let Some(c) = current {
            push(c);
        }
        push("hicolor");
        let mut others: Vec<String> = roots
            .iter()
            .filter_map(|r| std::fs::read_dir(r).ok())
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        others.sort();
        others.dedup();
        for o in others {
            push(&o);
        }
        for t in &themes {
            if let Some(p) = in_theme(t, icon) {
                return Some(p);
            }
        }
        for r in roots {
            let Some(parent) = r.parent() else { continue };
            for ext in ["png", "svg", "xpm"] {
                let p = parent.join("pixmaps").join(format!("{icon}.{ext}"));
                if p.is_file() && ext != "xpm" {
                    return Some(p);
                }
            }
        }
        None
    }

    pub fn lookup(n: &str) -> Option<super::Found> {
        let entry = find(&app_dirs(), n)?;
        let icon = find_icon(&entry.icon, &icon_roots(), current_theme().as_deref())
            .and_then(|path| std::fs::read(&path).ok())
            // The system's own file: an SVG of a theme is taken as it is.
            .and_then(|bytes| trusted(&bytes).or_else(|| normalize(&bytes).ok()));
        Some(super::Found {
            icon,
            label: super::clean_label(&entry.name),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn write(path: &Path, text: &str) {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        #[test]
        fn a_program_is_found_by_exec_file_name_class_or_name() {
            let dir = tempfile::tempdir().unwrap();
            let apps = dir.path().join("applications");
            write(&apps.join("firefox.desktop"), "[Desktop Entry]\nType=Application\nName=Firefox Web Browser\nExec=firefox %u\nIcon=firefox\n");
            write(&apps.join("google-chrome.desktop"), "[Desktop Entry]\nType=Application\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\nIcon=google-chrome\nStartupWMClass=Google-chrome\n");
            write(&apps.join("org.gnome.Nautilus.desktop"), "[Desktop Entry]\nType=Application\nName=Files\nExec=nautilus --new-window %U\nIcon=org.gnome.Nautilus\n");
            write(&apps.join("com.obsproject.Studio.desktop"), "[Desktop Entry]\nType=Application\nName=OBS Studio\nExec=env LIBGL=1 obs\nIcon=com.obsproject.Studio\n");
            write(&apps.join("flat.desktop"), "[Desktop Entry]\nType=Application\nName=Slack\nExec=flatpak run --branch=stable --arch=x86_64 com.slack.Slack\nIcon=com.slack.Slack\n");
            write(&apps.join("link.desktop"), "[Desktop Entry]\nType=Link\nName=Nope\nIcon=nope\n");
            write(&apps.join("noicon.desktop"), "[Desktop Entry]\nType=Application\nName=Quiet\nExec=quiet\n");
            let dirs = vec![apps];
            let icon = |n: &str| find(&dirs, n).map(|e| e.icon);
            assert_eq!(icon("firefox").as_deref(), Some("firefox"));
            // The process is `chrome`, the program `google-chrome-stable`.
            assert_eq!(icon("chrome").as_deref(), Some("google-chrome"));
            assert_eq!(icon("google-chrome").as_deref(), Some("google-chrome"));
            assert_eq!(icon("nautilus").as_deref(), Some("org.gnome.Nautilus"));
            assert_eq!(icon("obs").as_deref(), Some("com.obsproject.Studio"));
            assert_eq!(icon("com.slack.slack").as_deref(), Some("com.slack.Slack"));
            assert_eq!(icon("files").as_deref(), Some("org.gnome.Nautilus"));
            // A name that is only part of another word, a link, a file without an icon.
            assert_eq!(icon("fire"), None);
            assert_eq!(icon("nope"), None);
            assert_eq!(icon("quiet"), None);
        }

        #[test]
        fn the_display_name_is_the_desktop_files_and_is_made_safe() {
            let dir = tempfile::tempdir().unwrap();
            let apps = dir.path().join("applications");
            write(&apps.join("firefox.desktop"), "[Desktop Entry]\nType=Application\nName=Firefox\nName[zh_CN]=火狐\nExec=firefox %u\nIcon=firefox\n");
            write(&apps.join("code.desktop"), "[Desktop Entry]\nType=Application\nName=Visual Studio Code\nExec=/usr/share/code/code %F\nIcon=vscode\n");
            let dirs = vec![apps];
            let name = |n: &str| find(&dirs, n).and_then(|e| super::super::clean_label(&e.name));
            assert_eq!(name("firefox").as_deref(), Some("Firefox"));
            assert_eq!(name("code").as_deref(), Some("Visual Studio Code"));
            assert_eq!(name("nothing"), None);
            assert_eq!(super::super::clean_label("  A\nB\t C  ").as_deref(), Some("A B C"));
            assert_eq!(super::super::clean_label("   "), None);
            assert_eq!(super::super::clean_label(&"x".repeat(100)).unwrap().len(), 60);
        }

        /// A Chrome web app ("Linear") has its own entry whose `Exec` is Chrome's own `chrome`
        /// binary: it must never be taken for the browser (the file names are a real machine's).
        #[test]
        fn a_chrome_web_app_is_never_the_browser() {
            let dir = tempfile::tempdir().unwrap();
            let apps = dir.path().join("applications");
            let app_id = "bgdbmehlmdmddlgneophbcddadgknlpm";
            write(
                &apps.join(format!("chrome-{app_id}-Default.desktop")),
                &format!("[Desktop Entry]\nType=Application\nName=Linear\nExec=/home/u/.agent-browser/browsers/chrome-154.0.8037.92/chrome --user-data-dir=/tmp/p --profile-directory=Default --app-id={app_id}\nIcon=/icons/chrome-{app_id}-Default.png\nStartupWMClass=crx_{app_id}\n"),
            );
            write(&apps.join("google-chrome.desktop"), "[Desktop Entry]\nType=Application\nName=Google Chrome\nStartupWMClass=Google-chrome\nExec=/usr/bin/google-chrome-stable %U\nIcon=google-chrome\n[Desktop Action new-window]\nName=New Window\nExec=/usr/bin/google-chrome-stable\n");
            write(&apps.join("com.google.Chrome.desktop"), "[Desktop Entry]\nType=Application\nName=Google Chrome\nExec=/usr/bin/google-chrome-stable %U\nIcon=google-chrome\n");
            for other in ["msedge-mjoklplbddabcmpepnokjaffbmgbkjal-Default", "brave-fmpnliohjhemenmnlpbfmorieiaiabbc-Default", "chromium-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-Profile 1"] {
                let browser = other.split('-').next().unwrap();
                write(
                    &apps.join(format!("{other}.desktop")),
                    &format!("[Desktop Entry]\nType=Application\nName=Some App\nExec=/opt/{browser}/{browser} --app-id=x\nIcon=/icons/{browser}-app.png\n"),
                );
            }
            let dirs = vec![apps];
            let icon = |n: &str| find(&dirs, n).map(|e| e.icon);
            assert_eq!(icon("chrome").as_deref(), Some("google-chrome"));
            assert_eq!(icon("google-chrome").as_deref(), Some("google-chrome"));
            for browser in ["msedge", "brave", "chromium"] {
                let got = icon(browser);
                assert!(got.is_none() || !got.unwrap().contains("-app.png"), "{browser}: a web app is not the browser");
            }
            // The web app itself is still found by its own name and its own window class.
            assert_eq!(icon("linear"), Some(format!("/icons/chrome-{app_id}-Default.png")));
            assert_eq!(icon(&format!("crx_{app_id}")), Some(format!("/icons/chrome-{app_id}-Default.png")));
        }

        #[test]
        fn an_icon_is_found_in_the_desktops_theme_then_hicolor_then_the_rest() {
            let dir = tempfile::tempdir().unwrap();
            let icons = dir.path().join("icons");
            for p in [
                "hicolor/16x16/apps/foo.png",
                "hicolor/48x48/apps/foo.png",
                "hicolor/256x256/apps/foo.png",
                "hicolor/scalable/apps/foo.svg",
                "Papirus/64x64/apps/bar.svg",
                "Papirus/apps/64/foo.svg",
                "Other/32x32/apps/baz.png",
            ] {
                write(&icons.join(p), "x");
            }
            let roots = vec![icons.clone()];
            // A PNG of 64 to 256 points, the nearest to 128.
            assert_eq!(find_icon("foo", &roots, None), Some(icons.join("hicolor/256x256/apps/foo.png")));
            // The desktop's theme comes first, in either layout.
            assert_eq!(find_icon("foo", &roots, Some("Papirus")), Some(icons.join("Papirus/apps/64/foo.svg")));
            assert_eq!(find_icon("bar", &roots, None), Some(icons.join("Papirus/64x64/apps/bar.svg")));
            // Only small ones: the biggest.
            assert_eq!(find_icon("baz", &roots, None), Some(icons.join("Other/32x32/apps/baz.png")));
            assert_eq!(find_icon("missing", &roots, None), None);
            // A path is itself.
            let file = icons.join("hicolor/16x16/apps/foo.png");
            assert_eq!(find_icon(file.to_str().unwrap(), &roots, None), Some(file));
        }
    }
}

// --- macOS -------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{Found, normalize};

    fn app_folders() -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = vec![
            "/Applications".into(),
            "/Applications/Utilities".into(),
            "/System/Applications".into(),
            "/System/Applications/Utilities".into(),
        ];
        if let Some(h) = std::env::var_os("HOME") {
            out.push(Path::new(&h).join("Applications"));
        }
        out
    }

    fn bundles() -> Vec<PathBuf> {
        app_folders()
            .into_iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
            .collect()
    }

    /// `Info.plist` as JSON (`plutil` reads both the XML and the binary form).
    fn info(bundle: &Path) -> Option<serde_json::Value> {
        let out = Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(bundle.join("Contents/Info.plist"))
            .output()
            .ok()?;
        serde_json::from_slice(&out.stdout).ok()
    }

    fn icns_of(bundle: &Path, info: &serde_json::Value) -> Option<PathBuf> {
        let file = info.get("CFBundleIconFile")?.as_str()?;
        let file = if file.ends_with(".icns") { file.to_owned() } else { format!("{file}.icns") };
        let p = bundle.join("Contents/Resources").join(file);
        p.is_file().then_some(p)
    }

    pub fn lookup(n: &str) -> Option<Found> {
        let all = bundles();
        // By the bundle's name first, then by the program it runs.
        let by_name = all.iter().find(|b| {
            b.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase() == n)
        });
        let (bundle, plist) = match by_name {
            Some(b) => (b.clone(), info(b)?),
            None => all.iter().find_map(|b| {
                let i = info(b)?;
                let exe = i.get("CFBundleExecutable")?.as_str()?.to_lowercase();
                (exe == n).then(|| (b.clone(), i))
            })?,
        };
        // The name the Finder shows: the display name, else the bundle's name, else its file's.
        let label = ["CFBundleDisplayName", "CFBundleName"]
            .iter()
            .find_map(|k| plist.get(k)?.as_str().map(str::to_owned))
            .or_else(|| bundle.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .and_then(|l| super::clean_label(&l));
        let icon = icns_of(&bundle, &plist).and_then(|icns| {
            let tmp = std::env::temp_dir().join(format!("kairo-icon-{}.png", std::process::id()));
            let ok = Command::new("/usr/bin/sips")
                .args(["-s", "format", "png", "-Z", "128"])
                .arg(&icns)
                .arg("--out")
                .arg(&tmp)
                .output()
                .ok()
                .is_some_and(|o| o.status.success());
            let bytes = ok.then(|| std::fs::read(&tmp).ok()).flatten();
            let _ = std::fs::remove_file(&tmp);
            normalize(&bytes?).ok()
        });
        Some(Found { icon, label })
    }
}

// --- Windows -----------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use super::Found;

    /// No console window for the helper.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub fn lookup(n: &str) -> Option<Found> {
        // A running program of this name: its executable's icon and the name it gives itself.
        let proc = crate::bypass::list_processes()
            .into_iter()
            .find(|p| super::program_name(&p.name) == n)?;
        let out = std::env::temp_dir().join(format!("kairo-icon-{}.png", std::process::id()));
        let label_out = std::env::temp_dir().join(format!("kairo-icon-{}.txt", std::process::id()));
        // `ExtractAssociatedIcon` is Windows' own: the icon Explorer shows for the file. The paths
        // go in the environment, so nothing of a file name is ever part of the script.
        let script = "Add-Type -AssemblyName System.Drawing; \
             $i = [System.Drawing.Icon]::ExtractAssociatedIcon($env:KAIRO_ICON_EXE); \
             if ($i -ne $null) { $i.ToBitmap().Save($env:KAIRO_ICON_OUT, [System.Drawing.Imaging.ImageFormat]::Png) }; \
             $d = (Get-Item -LiteralPath $env:KAIRO_ICON_EXE).VersionInfo.FileDescription; \
             if ($d) { [System.IO.File]::WriteAllText($env:KAIRO_ICON_LABEL, $d, (New-Object System.Text.UTF8Encoding $false)) }";
        let ok = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("KAIRO_ICON_EXE", &proc.path)
            .env("KAIRO_ICON_OUT", &out)
            .env("KAIRO_ICON_LABEL", &label_out)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .is_some_and(|o| o.status.success());
        let bytes = ok.then(|| std::fs::read(&out).ok()).flatten();
        let _ = std::fs::remove_file(&out);
        let label = ok
            .then(|| std::fs::read_to_string(&label_out).ok())
            .flatten()
            .and_then(|l| super::clean_label(&l));
        let _ = std::fs::remove_file(&label_out);
        let icon = bytes.and_then(|b| super::normalize(&b).ok());
        (icon.is_some() || label.is_some()).then_some(Found { icon, label })
    }
}
