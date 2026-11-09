//! The macOS window, built from AppKit's parts around the NSH content:
//!
//! - an `NSSplitViewController` whose first item is a sidebar item: AppKit
//!   draws it as the floating Liquid Glass sidebar (macOS 26 and later; the
//!   translucent source list before);
//! - a source list (`NSTableView`, the sidebar style) of the pages, with SF
//!   Symbols;
//! - a unified `NSToolbar` with the sidebar's tracking separator: the page's
//!   title and the core's state are the window's title and subtitle;
//! - the NSH view as the content item's view.
//!
//! NSH's `run_app` made the window and keeps owning its life (close, reopen,
//! quit); this rebuilds its content once it is on screen.

use std::cell::{Cell, RefCell};

use kairo_core::{CoreStatus, Service};
use nsh_app_kairo::chrome;
use nsh_app_kairo::store::Page;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSColor, NSControlTextEditingDelegate, NSFont, NSImage, NSImageView, NSLayoutConstraint,
    NSScrollView, NSSearchToolbarItem, NSSplitViewController, NSSplitViewItem, NSTableCellView, NSTableColumn,
    NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle,
    NSTextField, NSToolbar, NSToolbarDelegate, NSToolbarItem, NSToolbarItemIdentifier, NSView, NSViewController,
    NSWindow, NSWindowStyleMask, NSWindowTitleVisibility, NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSIndexSet, NSInteger, NSNotification, NSObjectProtocol, NSString, ns_string};

/// The sidebar's rows: a section heading or a page.
#[derive(Clone, Copy)]
enum Row {
    Heading(&'static str),
    Item(Page),
}

const ROWS: &[Row] = &[
    Row::Item(Page::Overview),
    Row::Item(Page::Proxies),
    Row::Item(Page::Profiles),
    Row::Heading("Activity"),
    Row::Item(Page::Connections),
    Row::Item(Page::Rules),
    Row::Item(Page::Logs),
    Row::Item(Page::Dns),
    Row::Heading(""),
    Row::Item(Page::Settings),
];

fn symbol(page: Page) -> &'static str {
    match page {
        Page::Overview => "gauge.with.dots.needle.33percent",
        Page::Proxies => "globe",
        Page::Profiles => "doc.text",
        Page::Connections => "arrow.up.arrow.down",
        Page::Rules => "line.3.horizontal.decrease",
        Page::Logs => "text.alignleft",
        Page::Dns => "server.rack",
        Page::Settings => "gearshape",
    }
}

fn row_of(page: Page) -> usize {
    ROWS.iter().position(|r| matches!(r, Row::Item(p) if *p == page)).unwrap_or(0)
}

pub struct SidebarIvars {
    /// Set while the app moves the selection, so the change is not echoed.
    quiet: Cell<bool>,
    table: RefCell<Option<Retained<NSTableView>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SidebarIvars]
    #[name = "KairoSidebar"]
    pub struct Sidebar;

    unsafe impl NSObjectProtocol for Sidebar {}
    unsafe impl NSControlTextEditingDelegate for Sidebar {}

    unsafe impl NSTableViewDataSource for Sidebar {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn rows(&self, _table: &NSTableView) -> NSInteger {
            ROWS.len() as NSInteger
        }
    }

    unsafe impl NSTableViewDelegate for Sidebar {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(&self, _table: &NSTableView, _column: Option<&NSTableColumn>, row: NSInteger) -> Option<Retained<NSView>> {
            let mtm = self.mtm();
            Some(match ROWS[row as usize] {
                Row::Heading(title) => {
                    let label = NSTextField::labelWithString(&NSString::from_str(title), mtm);
                    label.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::smallSystemFontSize())));
                    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                    let cell = NSTableCellView::new(mtm);
                    pin_label(&cell, &label, 2.0);
                    // SAFETY: the label is the cell's subview.
                    unsafe { cell.setTextField(Some(&label)) };
                    Retained::into_super(cell)
                }
                Row::Item(page) => item_cell(mtm, page),
            })
        }

        #[unsafe(method(tableView:isGroupRow:))]
        fn is_group(&self, _table: &NSTableView, row: NSInteger) -> bool {
            matches!(ROWS[row as usize], Row::Heading(_))
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        fn should_select(&self, _table: &NSTableView, row: NSInteger) -> bool {
            matches!(ROWS[row as usize], Row::Item(_))
        }

        #[unsafe(method(tableView:heightOfRow:))]
        fn height(&self, _table: &NSTableView, row: NSInteger) -> f64 {
            match ROWS[row as usize] {
                Row::Heading("") => 10.0,
                Row::Heading(_) => 26.0,
                Row::Item(_) => 28.0,
            }
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _n: &NSNotification) {
            if self.ivars().quiet.get() {
                return;
            }
            let Some(table) = self.ivars().table.borrow().clone() else { return };
            let row = table.selectedRow();
            if let Some(Row::Item(page)) = usize::try_from(row).ok().and_then(|r| ROWS.get(r)) {
                chrome::select(*page);
            }
        }
    }
);

impl Sidebar {
    fn select(&self, page: Page) {
        let Some(table) = self.ivars().table.borrow().clone() else { return };
        self.ivars().quiet.set(true);
        table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row_of(page)), false);
        self.ivars().quiet.set(false);
    }
}

fn pin_label(cell: &NSView, label: &NSTextField, leading: f64) {
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    cell.addSubview(label);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        label.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), leading),
        label.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&cell.trailingAnchor(), -4.0),
        label.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
    ]));
}

fn item_cell(mtm: MainThreadMarker, page: Page) -> Retained<NSView> {
    let cell = NSTableCellView::new(mtm);
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(symbol(page)), None);
    let icon = NSImageView::new(mtm);
    icon.setImage(image.as_deref());
    icon.setTranslatesAutoresizingMaskIntoConstraints(false);
    cell.addSubview(&icon);
    let label = NSTextField::labelWithString(&NSString::from_str(page.title()), mtm);
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    cell.addSubview(&label);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        icon.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 4.0),
        icon.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
        icon.widthAnchor().constraintEqualToConstant(18.0),
        label.leadingAnchor().constraintEqualToAnchor_constant(&icon.trailingAnchor(), 7.0),
        label.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&cell.trailingAnchor(), -4.0),
        label.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
    ]));
    // SAFETY: both are the cell's subviews.
    unsafe {
        cell.setImageView(Some(&icon));
        cell.setTextField(Some(&label));
    }
    Retained::into_super(cell)
}

// --- Toolbar ------------------------------------------------------------------------

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "KairoToolbar"]
    pub struct Toolbar;

    unsafe impl NSObjectProtocol for Toolbar {}

    unsafe impl NSToolbarDelegate for Toolbar {
        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn defaults(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            identifiers()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            identifiers()
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(&self, _toolbar: &NSToolbar, id: &NSToolbarItemIdentifier, _insert: bool) -> Option<Retained<NSToolbarItem>> {
            let mtm = self.mtm();
            (id.to_string() == "kairo.search").then(|| {
                let item = NSSearchToolbarItem::initWithItemIdentifier(NSSearchToolbarItem::alloc(mtm), id);
                Retained::into_super(item)
            })
        }
    }
);

fn identifiers() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    // SAFETY: AppKit's constant identifiers.
    unsafe {
        NSArray::from_slice(&[
            objc2_app_kit::NSToolbarToggleSidebarItemIdentifier,
            objc2_app_kit::NSToolbarSidebarTrackingSeparatorItemIdentifier,
            objc2_app_kit::NSToolbarFlexibleSpaceItemIdentifier,
            ns_string!("kairo.search"),
        ])
    }
}

// --- The window ---------------------------------------------------------------------

struct Installed {
    window: Retained<NSWindow>,
    sidebar: Retained<Sidebar>,
    _toolbar: Retained<Toolbar>,
    _split: Retained<NSSplitViewController>,
}

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

fn log_class(name: &std::ffi::CStr) {
    log::warn!("spike: class {:?} {}", name, if objc2::runtime::AnyClass::get(name).is_some() { "present" } else { "MISSING" });
}

/// Rebuilds the main window's content once `run_app` shows it.
pub fn install(svc: Service) {
    dispatch2::DispatchQueue::main().exec_async(move || {
        let Some(mtm) = MainThreadMarker::new() else { return };
        log_class(c"NSGlassEffectView");
        log_class(c"NSGlassEffectContainerView");
        log_class(c"NSBackgroundExtensionView");
        let app = NSApplication::sharedApplication(mtm);
        let Some(window) = app.windows().iter().find(|w| w.styleMask().contains(NSWindowStyleMask::Resizable)) else {
            log::warn!("spike: no main window");
            return;
        };
        let Some(nsh) = window.contentView() else { return };
        let frame = window.frame();
        let under_toolbar = std::env::var("KAIRO_SPIKE").as_deref() == Ok("under");

        // Sidebar: a source list in a scroll view.
        let table = NSTableView::new(mtm);
        table.setStyle(NSTableViewStyle::SourceList);
        table.setHeaderView(None);
        table.setFloatsGroupRows(false);
        let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("page"));
        table.addTableColumn(&column);
        let sidebar = Sidebar::alloc(mtm).set_ivars(SidebarIvars { quiet: Cell::new(false), table: RefCell::new(Some(table.clone())) });
        // SAFETY: NSObject's initializer.
        let sidebar: Retained<Sidebar> = unsafe { msg_send![super(sidebar), init] };
        // SAFETY: the sidebar is kept alive in INSTALLED for the window's life.
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(&*sidebar)));
            table.setDelegate(Some(ProtocolObject::from_ref(&*sidebar)));
        }
        let scroll = NSScrollView::new(mtm);
        scroll.setDocumentView(Some(&table));
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let sidebar_vc = NSViewController::new(mtm);
        sidebar_vc.setView(&scroll);

        // Content: the NSH view, below the toolbar or under it.
        let container = NSView::new(mtm);
        nsh.removeFromSuperview();
        nsh.setTranslatesAutoresizingMaskIntoConstraints(false);
        container.addSubview(&nsh);
        let top = if under_toolbar {
            nsh.topAnchor().constraintEqualToAnchor(&container.topAnchor())
        } else {
            nsh.topAnchor().constraintEqualToAnchor(&container.safeAreaLayoutGuide().topAnchor())
        };
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            top,
            nsh.leadingAnchor().constraintEqualToAnchor(&container.leadingAnchor()),
            nsh.trailingAnchor().constraintEqualToAnchor(&container.trailingAnchor()),
            nsh.bottomAnchor().constraintEqualToAnchor(&container.bottomAnchor()),
        ]));
        let content_vc = NSViewController::new(mtm);
        content_vc.setView(&container);

        let split = NSSplitViewController::new(mtm);
        let side_item = NSSplitViewItem::sidebarWithViewController(&sidebar_vc);
        side_item.setMinimumThickness(190.0);
        side_item.setMaximumThickness(280.0);
        side_item.setCanCollapse(true);
        let content_item = NSSplitViewItem::splitViewItemWithViewController(&content_vc);
        content_item.setMinimumThickness(560.0);
        split.addSplitViewItem(&side_item);
        split.addSplitViewItem(&content_item);

        window.setStyleMask(window.styleMask() | NSWindowStyleMask::FullSizeContentView);
        window.setTitlebarAppearsTransparent(false);
        window.setTitleVisibility(NSWindowTitleVisibility::Visible);
        window.setContentViewController(Some(&split));
        window.setFrame_display(frame, true);

        let toolbar_delegate: Retained<Toolbar> = unsafe { msg_send![Toolbar::alloc(mtm), init] };
        let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), ns_string!("kairo.main"));
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*toolbar_delegate)));
        toolbar.setDisplayMode(objc2_app_kit::NSToolbarDisplayMode::IconOnly);
        window.setToolbar(Some(&toolbar));
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        window.makeFirstResponder(Some(&nsh));

        let page = *chrome::bridge().page.borrow();
        sidebar.select(page);
        window.setTitle(&NSString::from_str(page.title()));
        table.sizeLastColumnToFit();

        INSTALLED.with(|i| {
            *i.borrow_mut() = Some(Installed { window: window.clone(), sidebar, _toolbar: toolbar_delegate, _split: split })
        });
        log::warn!("spike: chrome installed (under_toolbar={under_toolbar})");
        follow(svc);
    });
}

/// Keeps the chrome in step: the page (title, selection) and the core's state
/// (subtitle).
fn follow(svc: Service) {
    let mut pages = chrome::bridge().page.subscribe();
    let mut state = svc.state();
    let mut live = svc.live();
    std::thread::Builder::new()
        .name("kairo-chrome".into())
        .spawn(move || {
            let rt = kairo_core::tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
            rt.block_on(async move {
                loop {
                    let page = *pages.borrow_and_update();
                    let s = state.borrow_and_update().clone();
                    let l = live.borrow_and_update().clone();
                    let subtitle = match &s.core {
                        CoreStatus::Running { .. } => {
                            format!("{} · {} · ↓ {}  ↑ {}", s.core.title(), s.settings.mode.title(), kairo_core::speed(l.down), kairo_core::speed(l.up))
                        }
                        other => other.title().to_owned(),
                    };
                    let theme = s.settings.theme;
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        INSTALLED.with(|i| {
                            if let Some(i) = i.borrow().as_ref() {
                                set_appearance(&i.window, theme);
                                i.window.setTitle(&NSString::from_str(page.title()));
                                i.window.setSubtitle(&NSString::from_str(&subtitle));
                                i.sidebar.select(page);
                            }
                        })
                    });
                    kairo_core::tokio::select! {
                        r = pages.changed() => if r.is_err() { break },
                        r = state.changed() => if r.is_err() { break },
                        r = live.changed() => if r.is_err() { break },
                    }
                }
            });
        })
        .expect("chrome thread");
}

/// The window's appearance (and so the sidebar's, the toolbar's and the
/// controls') follows Kairo's own setting; System follows the Mac.
fn set_appearance(window: &NSWindow, theme: kairo_core::Theme) {
    use objc2_app_kit::{NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua};
    // SAFETY: AppKit's constant appearance names.
    let name = match theme {
        kairo_core::Theme::System => None,
        kairo_core::Theme::Light => Some(unsafe { NSAppearanceNameAqua }),
        kairo_core::Theme::Dark => Some(unsafe { NSAppearanceNameDarkAqua }),
    };
    let appearance = name.and_then(NSAppearance::appearanceNamed);
    window.setAppearance(appearance.as_deref());
}

// --- Native controls in the content ---------------------------------------------------

pub mod controls {
    use std::rc::Rc;

    use nsh_platform_macos::{ComponentContext, Components, NativeComponent};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
    use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn, NSSegmentedControl, NSSegmentStyle, NSSwitch, NSView};
    use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
    use serde_json::Value;

    define_class!(
        /// Reports a control's action to its component's context.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[ivars = ComponentContext]
        #[name = "KairoControlTarget"]
        struct Target;

        unsafe impl NSObjectProtocol for Target {}

        impl Target {
            #[unsafe(method(switched:))]
            fn switched(&self, sender: &NSSwitch) {
                let on = sender.state() == NSControlStateValueOn;
                self.ivars().emit("change", serde_json::json!({ "on": on }));
            }

            #[unsafe(method(segmented:))]
            fn segmented(&self, sender: &NSSegmentedControl) {
                self.ivars().emit("change", serde_json::json!({ "index": sender.selectedSegment() }));
            }
        }
    );

    fn target(mtm: MainThreadMarker, ctx: ComponentContext) -> Retained<Target> {
        let t = Target::alloc(mtm).set_ivars(ctx);
        // SAFETY: NSObject's initializer.
        unsafe { msg_send![super(t), init] }
    }

    struct Switch(Retained<NSSwitch>, #[allow(dead_code)] Retained<Target>);

    impl NativeComponent for Switch {
        fn view(&self) -> Retained<NSView> {
            Retained::into_super(Retained::into_super(self.0.clone()))
        }
        fn set_prop(&mut self, name: &str, value: Option<&Value>) {
            match name {
                "on" => self.0.setState(if value.and_then(Value::as_bool).unwrap_or(false) { NSControlStateValueOn } else { NSControlStateValueOff }),
                "enabled" => self.0.setEnabled(value.and_then(Value::as_bool).unwrap_or(true)),
                _ => {}
            }
        }
    }

    struct Segmented(Retained<NSSegmentedControl>, #[allow(dead_code)] Retained<Target>);

    impl NativeComponent for Segmented {
        fn view(&self) -> Retained<NSView> {
            Retained::into_super(Retained::into_super(self.0.clone()))
        }
        fn set_prop(&mut self, name: &str, value: Option<&Value>) {
            match name {
                "labels" => {
                    let labels: Vec<&str> = value.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                    self.0.setSegmentCount(labels.len() as isize);
                    for (i, l) in labels.iter().enumerate() {
                        self.0.setLabel_forSegment(&NSString::from_str(l), i as isize);
                    }
                }
                "selected" => {
                    self.0.setSelectedSegment(value.and_then(Value::as_i64).unwrap_or(-1) as isize);
                }
                _ => {}
            }
        }
    }

    /// Registers `kairo/switch` and `kairo/segmented`.
    pub fn register(components: &Components) {
        components.register(
            "kairo/switch",
            Rc::new(|ctx: ComponentContext| {
                let mtm = MainThreadMarker::new().expect("main thread");
                let s = NSSwitch::new(mtm);
                let t = target(mtm, ctx);
                // SAFETY: the target lives as long as the component.
                unsafe {
                    s.setTarget(Some(&*(Retained::as_ptr(&t) as *const AnyObject)));
                    s.setAction(Some(sel!(switched:)));
                }
                Box::new(Switch(s, t)) as Box<dyn NativeComponent>
            }),
        );
        components.register(
            "kairo/segmented",
            Rc::new(|ctx: ComponentContext| {
                let mtm = MainThreadMarker::new().expect("main thread");
                let s = NSSegmentedControl::new(mtm);
                s.setSegmentStyle(NSSegmentStyle::Automatic);
                let t = target(mtm, ctx);
                // SAFETY: the target lives as long as the component.
                unsafe {
                    s.setTarget(Some(&*(Retained::as_ptr(&t) as *const AnyObject)));
                    s.setAction(Some(sel!(segmented:)));
                }
                Box::new(Segmented(s, t)) as Box<dyn NativeComponent>
            }),
        );
    }
}
