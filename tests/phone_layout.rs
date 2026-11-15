//! Kairo on a phone (Android, 412 points wide), laid out headless: what the
//! owner's phone showed broken must not come back.
//!
//! - Subscriptions: a failed update's card gives the provider's message the
//!   card's width (it was a column a few characters wide beside Retry), cuts a
//!   long one at three lines with Show All, and puts Retry under the words.
//! - Apps: the kinds' cards fill the width in two equal columns (they filled
//!   two thirds), All Apps is a chip above them and not a card among them,
//!   and no kind's name is cut or missing ("流…", the AI card's empty name).
//!
//! The text engine is the headless one (every character 0.55 em), so the
//! checks are of the layout, not of a platform's fonts. One test per binary:
//! the service and `KAIRO_HOME` are the process'.

use std::time::Duration;

use nsh_app_kairo::App;
use nsh_core::platform::{Idiom, Os};
use nsh_core::prelude::{px, size};
use nsh_core::{ApproxTextEngine, TextEngine};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode};

const WIDTH: f32 = 412.0;

/// The provider's words from the owner's screenshot (Kairo keeps them as its
/// "Your provider says: …" error).
const NOTICE: &str = "Your provider says: 请登录系统网站后台，或在系统网站后台的订阅(SS/Trojan)页面开启订阅获取。每次开启后，订阅可获取有效时间为：10 分钟";

/// A provider that says far more than three lines.
const SPEECH: &str = "Your provider says: Your plan has expired. To keep using our service, sign in to the website, open the Plans page, choose a plan and pay for it; then open Subscriptions and press Enable Fetching. Fetching stays enabled for ten minutes after you press it. If you need help, write to our support team from the Tickets page, and we answer within a day.";

fn nodes(app: &Headless) -> Vec<LayoutNode> {
    app.screen().layout_nodes()
}

fn text_of(n: &LayoutNode) -> Option<&str> {
    n.text.as_ref().map(|t| t.text.text.as_str())
}

/// The first text view whose text `pred` accepts.
fn find(app: &Headless, what: &str, pred: impl Fn(&str) -> bool) -> LayoutNode {
    nodes(app)
        .into_iter()
        .find(|n| !n.hidden && text_of(n).is_some_and(&pred))
        .unwrap_or_else(|| panic!("no text on screen for {what}: {:?}", app.screen().texts()))
}

/// The view `n` is in, `up` levels up.
fn ancestor(app: &Headless, n: &LayoutNode, up: usize) -> LayoutNode {
    let all = nodes(app);
    let mut cur = n.clone();
    for _ in 0..up {
        cur = all
            .iter()
            .find(|p| p.id == cur.parent)
            .cloned()
            .expect("an ancestor");
    }
    cur
}

/// Whether the text of `n` is cut: it needs more lines at its width than its
/// line limit gives it.
fn cut(n: &LayoutNode) -> bool {
    let Some(t) = &n.text else { return false };
    if t.text.max_lines == 0 {
        return false;
    }
    let engine = ApproxTextEngine;
    let mut whole = (*t.text).clone();
    whole.max_lines = 0;
    let at = Some(t.content.size.width);
    let needed = engine.measure(&engine.prepare(&whole), at).height;
    let given = engine.measure(&engine.prepare(&t.text), at).height;
    needed.0 > given.0 + 0.5
}

fn right(n: &LayoutNode) -> f32 {
    (n.bounds.origin.x + n.bounds.size.width).0
}

fn bottom(n: &LayoutNode) -> f32 {
    (n.bounds.origin.y + n.bounds.size.height).0
}

#[test]
fn a_phone_gives_the_message_and_the_kinds_the_width() {
    let home = std::env::temp_dir().join(format!("kairo-phone-layout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // A subscription whose last update failed with the provider's notice. Its link
    // is a closed port, nothing updates it (fresh, every 12 hours).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let profiles = serde_json::json!({
        "active": "phone0layout",
        "profiles": [{
            "id": "phone0layout",
            "name": "Nexitally_Clash",
            "source": {"type": "remote", "url": "http://127.0.0.1:9/sub", "intervalHours": 12},
            "updatedAt": now,
            "lastError": NOTICE,
            "failedAt": now,
            "failureDismissed": false,
        }, {
            "id": "phone1layout",
            "name": "Backup",
            "source": {"type": "remote", "url": "http://127.0.0.1:9/backup", "intervalHours": 12},
            "updatedAt": now,
            "lastError": SPEECH,
            "failedAt": now,
            "failureDismissed": false,
        }]
    });
    std::fs::write(home.join("profiles.json"), profiles.to_string()).unwrap();
    // SAFETY: set before anything reads them, in a test binary with one test.
    unsafe {
        std::env::set_var("KAIRO_HOME", &home);
        std::env::set_var("KAIRO_NO_BYPASS", "1");
        std::env::set_var("KAIRO_NO_SYSPROXY", "1");
        std::env::set_var("KAIRO_NO_UPDATE", "1");
        std::env::set_var("KAIRO_NO_ICON_FETCH", "1");
    }
    let root = Root::of::<App>().translations(nsh_app_kairo::i18n::translations());
    let mut config = EngineConfig::new(root);
    config.environment.os = Os::Android;
    config.environment.idiom = Idiom::Phone;
    config.viewport = size(px(WIDTH), px(915.0));
    let mut app = Headless::start(config).unwrap();
    app.first_render().unwrap();

    // --- Subscriptions: the failure card ------------------------------------------------
    app.press("text:Subs").unwrap();
    app.wait_for("text:Nexitally_Clash couldn't update").unwrap();
    let title = find(&app, "the card's title", |t| t == "Nexitally_Clash couldn't update");
    let message = find(&app, "the provider's message", |t| t.contains("请登录"));
    let retry = find(&app, "Retry", |t| t == "Retry");
    // The words take the card's width (16 + 16 points of margin, 16 + 16 of padding).
    let room = WIDTH - 64.0;
    assert!(
        message.bounds.size.width.0 >= room - 1.0,
        "the message is {} points wide of {room}",
        message.bounds.size.width.0
    );
    assert!(title.bounds.origin.y.0 < message.bounds.origin.y.0, "the title is its first line");
    assert!(retry.bounds.origin.y.0 >= bottom(&message), "Retry goes under the message");
    // It fits in three lines here: no Show All for it.
    let lines = |n: &LayoutNode| n.text.as_ref().map(|t| t.text.max_lines);
    assert!(!cut(&message), "the notice is shown whole");
    // A longer one: three lines and Show All; Show All shows the rest, Show Less cuts it again.
    let long = |app: &Headless| find(app, "the long message", |t| t.contains("Your plan has expired"));
    let speech = long(&app);
    assert!(speech.bounds.size.width.0 >= room - 1.0);
    assert_eq!(lines(&speech), Some(3));
    assert!(cut(&speech), "a long message is cut at three lines");
    assert_eq!(app.screen().count("text:Show All").unwrap(), 1, "Show All only where words are cut");
    let short = speech.bounds.size.height.0;
    app.press("text:Show All").unwrap();
    app.wait(Duration::from_millis(100)).unwrap();
    let speech = long(&app);
    assert!(!cut(&speech), "Show All shows it whole");
    assert!(speech.bounds.size.height.0 > short);
    app.press("text:Show Less").unwrap();
    app.wait(Duration::from_millis(100)).unwrap();
    assert!(cut(&long(&app)));

    // --- Apps: the kinds' cards ---------------------------------------------------------
    app.press("text:Apps").unwrap();
    app.wait_for("text:All apps").unwrap();
    let kinds = ["AI", "Streaming", "Games", "Downloads", "Dev tools", "Everything else"];
    let all = find(&app, "All apps", |t| t == "All apps");
    let chip = ancestor(&app, &all, 1);
    let mut cards = Vec::new();
    for k in kinds {
        let name = find(&app, k, |t| t == k);
        assert!(name.bounds.size.width.0 > 0.0, "{k}: the name has no room");
        assert!(!cut(&name), "{k}: the name is cut at {} points", name.bounds.size.width.0);
        // The name, its line (with the health), the card.
        cards.push((k, ancestor(&app, &name, 2)));
    }
    let left = cards[0].1.bounds.origin.x.0;
    let width = cards[0].1.bounds.size.width.0;
    for (k, c) in &cards {
        assert!((c.bounds.size.width.0 - width).abs() < 0.5, "{k}: the columns are equal");
        assert!(chip.bounds.origin.y.0 < c.bounds.origin.y.0 && bottom(&chip) <= c.bounds.origin.y.0, "{k}: All apps is above the cards");
    }
    let columns: Vec<f32> = {
        let mut xs: Vec<f32> = cards.iter().map(|(_, c)| c.bounds.origin.x.0.round()).collect();
        xs.sort_by(f32::total_cmp);
        xs.dedup();
        xs
    };
    assert_eq!(columns.len(), 2, "two columns on a phone: {columns:?}");
    let far = cards.iter().map(|(_, c)| right(c)).fold(0.0, f32::max);
    assert!((left - 16.0).abs() < 1.0, "the grid starts at the margin: {left}");
    assert!((far - (WIDTH - 16.0)).abs() < 1.0, "the grid reaches the other margin: {far}");
    // Every card's lines are whole (the place, "Fastest · …  · 1 app").
    for (k, c) in &cards {
        for n in nodes(&app).iter().filter(|n| {
            n.text.is_some()
                && n.bounds.origin.x.0 >= c.bounds.origin.x.0
                && right(n) <= right(c) + 0.5
                && n.bounds.origin.y.0 >= c.bounds.origin.y.0
                && bottom(n) <= bottom(c) + 0.5
        }) {
            assert!(!cut(n), "{k}: {:?} is cut", text_of(n));
        }
    }
    let _ = std::fs::remove_dir_all(home);
}
