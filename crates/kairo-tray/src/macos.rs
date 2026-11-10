//! The menu bar item: an `NSStatusItem` whose menu is rebuilt each time it opens.

use std::cell::RefCell;
use std::time::Duration;

use kairo_core::{Mode, Service};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate, NSMenuItem,
    NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSSize, NSString};

use crate::{ExitId, Hooks, Model, act};

const TAG_MODE: isize = 1; // + index
const TAG_CORE: isize = 20;
const TAG_SHOW: isize = 21;
const TAG_QUIT: isize = 22;
const TAG_ALL: isize = 23;
const TAG_TEST: isize = 24;
const TAG_SETTINGS: isize = 25;
const TAG_FASTEST: isize = 26;
const TAG_REST: isize = 1000; // + index
const TAG_AI: isize = 2000; // + index

pub struct Ivars {
    svc: Service,
    hooks: Hooks,
    model: RefCell<Model>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KairoTrayTarget"]
    #[ivars = Ivars]
    pub struct Target;

    impl Target {
        #[unsafe(method(pick:))]
        fn pick(&self, item: &NSMenuItem) {
            let tag = item.tag();
            let iv = self.ivars();
            let model = iv.model.borrow().clone();
            let choice = |id: ExitId, i: isize| {
                model
                    .exits
                    .iter()
                    .find(|e| e.id == id)
                    .and_then(|e| e.choices.get(i as usize))
                    .map(|c| c.name.clone())
            };
            match tag {
                t if (TAG_MODE..TAG_MODE + 3).contains(&t) => act::mode(&iv.svc, Mode::ALL[(t - TAG_MODE) as usize]),
                TAG_CORE => act::connect(&iv.svc, !model.running),
                TAG_SHOW => (iv.hooks.show)(),
                TAG_QUIT => (iv.hooks.quit)(),
                TAG_ALL => (iv.hooks.page)("exits"),
                TAG_SETTINGS => (iv.hooks.page)("settings"),
                TAG_TEST => act::test_again(&iv.svc),
                TAG_FASTEST => act::locate(&iv.svc, String::new()),
                t if t >= TAG_AI => {
                    if let Some(name) = choice(ExitId::Ai, t - TAG_AI) {
                        act::ai_exit(&iv.svc, name);
                    }
                }
                t if t >= TAG_REST => {
                    if let Some(name) = choice(ExitId::Rest, t - TAG_REST) {
                        act::locate(&iv.svc, name);
                    }
                }
                _ => {}
            }
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            let model = Model::of(&self.ivars().svc);
            *self.ivars().model.borrow_mut() = model.clone();
            build(self, menu, &model);
        }
    }
);

thread_local! {
    static HELD: RefCell<Option<(Retained<NSStatusItem>, Retained<Target>, Retained<NSMenu>)>> = const { RefCell::new(None) };
}

fn item(
    target: &Target,
    mtm: MainThreadMarker,
    title: &str,
    tag: isize,
    enabled: bool,
    checked: Option<bool>,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(sel!(pick:)),
            &NSString::from_str(""),
        )
    };
    unsafe { item.setTarget(Some(target)) };
    item.setTag(tag);
    item.setEnabled(enabled);
    if let Some(on) = checked {
        item.setState(if on {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }
    item
}

/// The item with an SF Symbol before its title, as macOS 26's menus have.
fn symbol(item: Retained<NSMenuItem>, name: &str) -> Retained<NSMenuItem> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    );
    item.setImage(image.as_deref());
    item
}

fn build(target: &Target, menu: &NSMenu, m: &Model) {
    let mtm = MainThreadMarker::from(target);
    menu.removeAllItems();
    menu.addItem(&symbol(
        item(
            target,
            mtm,
            if m.running { "Connected" } else { &m.status },
            TAG_CORE,
            m.has_profile,
            Some(m.running),
        ),
        "power",
    ));
    if !m.ai.is_empty() {
        menu.addItem(&item(target, mtm, &m.ai, 0, false, None));
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    for e in &m.exits {
        let holder = item(target, mtm, &format!("{}: {}", e.title, e.place), 0, true, None);
        unsafe { holder.setAction(None) };
        let sub = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(e.title));
        sub.setAutoenablesItems(false);
        if let Some(on) = e.fastest {
            sub.addItem(&symbol(
                item(target, mtm, "Fastest", TAG_FASTEST, true, Some(on)),
                "bolt",
            ));
            sub.addItem(&NSMenuItem::separatorItem(mtm));
        }
        let base = if e.id == ExitId::Ai { TAG_AI } else { TAG_REST };
        for (i, c) in e.choices.iter().enumerate() {
            sub.addItem(&item(
                target,
                mtm,
                &Model::choice_label(c),
                base + i as isize,
                true,
                Some(c.checked),
            ));
        }
        sub.addItem(&NSMenuItem::separatorItem(mtm));
        sub.addItem(&item(target, mtm, "All Servers…", TAG_ALL, true, None));
        sub.addItem(&item(target, mtm, "Test Again", TAG_TEST, true, None));
        holder.setSubmenu(Some(&sub));
        menu.addItem(&holder);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(target, mtm, "Mode", 0, false, None));
    for (i, mode) in Mode::ALL.iter().enumerate() {
        menu.addItem(&item(
            target,
            mtm,
            Model::route_title(*mode),
            TAG_MODE + i as isize,
            true,
            Some(*mode == m.mode),
        ));
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&symbol(
        item(target, mtm, "Open Kairo", TAG_SHOW, true, None),
        "macwindow",
    ));
    menu.addItem(&symbol(
        item(target, mtm, "Settings…", TAG_SETTINGS, true, None),
        "gearshape",
    ));
    menu.addItem(&symbol(
        item(target, mtm, "Quit Kairo", TAG_QUIT, true, None),
        "xmark.circle",
    ));
}

fn glyph(mtm: MainThreadMarker, dim: bool) -> Option<Retained<NSImage>> {
    let _ = mtm;
    // A PNG at 2x (every macOS reads it; SVG data needs a recent one), used as
    // a template image so the menu bar tints it for light, dark and highlight.
    let svg = include_str!("../../../assets/glyph.svg");
    let svg = if dim {
        svg.replacen("<svg ", "<svg opacity=\"0.45\" ", 1)
    } else {
        svg.to_owned()
    };
    let png = crate::render_png(&svg, 36);
    let data = NSData::with_bytes(&png);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(18.0, 18.0));
    image.setTemplate(true);
    Some(image)
}

fn install(svc: Service, hooks: Hooks, mtm: MainThreadMarker) {
    let target: Retained<Target> = {
        let this = Target::alloc(mtm).set_ivars(Ivars {
            model: RefCell::new(Model::of(&svc)),
            svc: svc.clone(),
            hooks,
        });
        unsafe { msg_send![super(this), init] }
    };
    let bar = NSStatusBar::systemStatusBar();
    let status = bar.statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status.button(mtm) {
        button.setImage(glyph(mtm, true).as_deref());
        button.setToolTip(Some(&NSString::from_str("Kairo")));
    }
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Kairo"));
    menu.setAutoenablesItems(false);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    build(&target, &menu, &Model::of(&svc));
    status.setMenu(Some(&menu));
    HELD.with(|h| *h.borrow_mut() = Some((status, target, menu)));

    // Dim the glyph while the core is stopped.
    let rt = svc.runtime();
    rt.spawn(async move {
        let mut state = svc.state();
        let mut last = None;
        loop {
            let running = state.borrow().core.is_running();
            if last != Some(running) {
                last = Some(running);
                dispatch2::DispatchQueue::main().exec_async(move || {
                    let Some(mtm) = MainThreadMarker::new() else {
                        return;
                    };
                    HELD.with(|h| {
                        if let Some((status, _, _)) = h.borrow().as_ref()
                            && let Some(button) = status.button(mtm)
                        {
                            button.setImage(glyph(mtm, !running).as_deref());
                        }
                    });
                });
            }
            if state.changed().await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}

pub fn start(svc: Service, hooks: Hooks) -> Result<(), String> {
    match MainThreadMarker::new() {
        Some(mtm) => install(svc, hooks, mtm),
        None => dispatch2::DispatchQueue::main().exec_async(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                install(svc, hooks, mtm);
            }
        }),
    }
    Ok(())
}
