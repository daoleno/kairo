//! The menu bar item: an `NSStatusItem` whose menu is rebuilt each time it opens.

use std::cell::RefCell;
use std::time::Duration;

use kairo_core::{Mode, Service};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSData, NSSize, NSString};

use crate::{Hooks, Model, act};

const TAG_MODE: isize = 1; // + index
const TAG_SYSPROXY: isize = 10;
const TAG_TUN: isize = 11;
const TAG_CORE: isize = 20;
const TAG_SHOW: isize = 21;
const TAG_QUIT: isize = 22;
const TAG_NODE: isize = 1000; // + index

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
            match tag {
                t if (TAG_MODE..TAG_MODE + 3).contains(&t) => act::mode(&iv.svc, Mode::ALL[(t - TAG_MODE) as usize]),
                TAG_SYSPROXY => act::sysproxy(&iv.svc, !model.sysproxy),
                TAG_TUN => act::tun(&iv.svc, !model.tun),
                TAG_CORE => act::toggle_core(&iv.svc, model.running),
                TAG_SHOW => (iv.hooks.show)(),
                TAG_QUIT => (iv.hooks.quit)(),
                t if t >= TAG_NODE => {
                    if let Some((group, choices, _)) = &model.group
                        && let Some(c) = choices.get((t - TAG_NODE) as usize)
                    {
                        act::select(&iv.svc, group.clone(), c.name.clone());
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

fn item(target: &Target, mtm: MainThreadMarker, title: &str, tag: isize, enabled: bool, checked: Option<bool>) -> Retained<NSMenuItem> {
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
        item.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    }
    item
}

fn build(target: &Target, menu: &NSMenu, m: &Model) {
    let mtm = MainThreadMarker::from(target);
    menu.removeAllItems();
    let status = item(target, mtm, &m.status, 0, false, None);
    menu.addItem(&status);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    for (i, mode) in Mode::ALL.iter().enumerate() {
        menu.addItem(&item(target, mtm, &format!("{} Mode", mode.title()), TAG_MODE + i as isize, true, Some(*mode == m.mode)));
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(target, mtm, "System Proxy", TAG_SYSPROXY, m.sysproxy_supported, Some(m.sysproxy)));
    menu.addItem(&item(target, mtm, "TUN Mode", TAG_TUN, m.tun_allowed || m.tun, Some(m.tun)));
    if let Some((group, choices, now)) = &m.group {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let holder = item(target, mtm, &format!("{group}: {}", now.clone().unwrap_or_default()), 0, m.running, None);
        unsafe { holder.setAction(None) };
        let sub = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(group));
        sub.setAutoenablesItems(false);
        for (i, c) in choices.iter().enumerate() {
            let checked = now.as_deref() == Some(c.name.as_str());
            sub.addItem(&item(target, mtm, &Model::choice_label(c), TAG_NODE + i as isize, m.running, Some(checked)));
        }
        holder.setSubmenu(Some(&sub));
        menu.addItem(&holder);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(target, mtm, if m.running { "Stop Core" } else { "Start Core" }, TAG_CORE, m.running || m.can_start, None));
    menu.addItem(&item(target, mtm, "Show Kairo", TAG_SHOW, true, None));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(target, mtm, "Quit Kairo", TAG_QUIT, true, None));
}

fn glyph(mtm: MainThreadMarker, dim: bool) -> Option<Retained<NSImage>> {
    let _ = mtm;
    let svg = include_str!("../../../assets/glyph.svg");
    let svg = if dim { svg.replace("fill=\"#000\"", "fill=\"#000\" fill-opacity=\"0.45\"").replace("stroke=\"#000\"", "stroke=\"#000\" stroke-opacity=\"0.45\"") } else { svg.to_owned() };
    let data = NSData::with_bytes(svg.as_bytes());
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(18.0, 18.0));
    image.setTemplate(true);
    Some(image)
}

fn install(svc: Service, hooks: Hooks, mtm: MainThreadMarker) {
    let target: Retained<Target> = {
        let this = Target::alloc(mtm).set_ivars(Ivars { model: RefCell::new(Model::of(&svc)), svc: svc.clone(), hooks });
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
                    let Some(mtm) = MainThreadMarker::new() else { return };
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
