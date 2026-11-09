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
use nsh_app_kairo::chrome::{self, Tool, ToolEvent};
use nsh_app_kairo::store::Page;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSControlTextEditingDelegate, NSFont, NSMenu,
    NSMenuItem, NSMenuToolbarItem, NSSearchField, NSStackView, NSUserInterfaceItemIdentification, NSImage, NSImageView, NSLayoutConstraint,
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

/// The fixed start of the toolbar: the sidebar's button and its tracking
/// separator (the title sits after it, over the content), then a flexible space.
const FIXED: usize = 3;

thread_local! {
    /// The tools the toolbar shows now (the delegate makes items from them).
    static TOOLS: RefCell<Vec<Tool>> = const { RefCell::new(Vec::new()) };
}

fn tool_id(id: &NSString) -> Option<&'static str> {
    let id = id.to_string();
    let id = id.strip_prefix("tool.")?;
    TOOLS.with(|t| t.borrow().iter().map(Tool::id).find(|t| *t == id))
}

fn symbol_image(name: &str, label: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), Some(&NSString::from_str(label)))
}

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
            let mut ids: Vec<Retained<NSString>> = identifiers().to_vec();
            TOOLS.with(|t| ids.extend(t.borrow().iter().map(|t| NSString::from_str(&format!("tool.{}", t.id())))));
            NSArray::from_retained_slice(&ids)
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(&self, _toolbar: &NSToolbar, id: &NSToolbarItemIdentifier, _insert: bool) -> Option<Retained<NSToolbarItem>> {
            let tool = tool_id(id).and_then(|tid| TOOLS.with(|t| t.borrow().iter().find(|t| t.id() == tid).cloned()));
            tool.map(|tool| make_item(self, id, &tool))
        }
    }

    impl Toolbar {
        #[unsafe(method(toolPressed:))]
        fn pressed(&self, item: &NSToolbarItem) {
            if let Some(id) = tool_id(&item.itemIdentifier()) {
                chrome::tool_event(ToolEvent::Pressed(id));
            }
        }

        #[unsafe(method(toolSearched:))]
        fn searched(&self, field: &NSSearchField) {
            if let Some(id) = field.identifier().and_then(|i| tool_id(&i)) {
                chrome::tool_event(ToolEvent::Search(id, field.stringValue().to_string()));
            }
        }

        #[unsafe(method(toolChose:))]
        fn chose(&self, item: &NSMenuItem) {
            if let Some(id) = item.identifier().and_then(|i| tool_id(&i)) {
                chrome::tool_event(ToolEvent::Chose(id, item.tag() as usize));
            }
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
        ])
    }
}

fn target(t: &Toolbar) -> &AnyObject {
    // SAFETY: an NSObject subclass is an object.
    unsafe { &*(t as *const Toolbar as *const AnyObject) }
}

fn make_item(delegate: &Toolbar, id: &NSString, tool: &Tool) -> Retained<NSToolbarItem> {
    let mtm = delegate.mtm();
    let item: Retained<NSToolbarItem> = match tool {
        Tool::Search { placeholder, .. } => {
            let item = NSSearchToolbarItem::initWithItemIdentifier(NSSearchToolbarItem::alloc(mtm), id);
            let field = item.searchField();
            field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
            field.setIdentifier(Some(id));
            field.setSendsSearchStringImmediately(true);
            // SAFETY: the delegate outlives the toolbar's items (INSTALLED).
            unsafe {
                field.setTarget(Some(target(delegate)));
                field.setAction(Some(sel!(toolSearched:)));
            }
            item.setPreferredWidthForSearchField(220.0);
            Retained::into_super(item)
        }
        Tool::Button { .. } => {
            let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), id);
            item.setAutovalidates(false);
            // SAFETY: as above.
            unsafe {
                item.setTarget(Some(target(delegate)));
                item.setAction(Some(sel!(toolPressed:)));
            }
            item
        }
        Tool::Choice { .. } => {
            let item = NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm), id);
            item.setShowsIndicator(true);
            Retained::into_super(item)
        }
    };
    update_item(delegate, &item, tool);
    item
}

fn update_item(delegate: &Toolbar, item: &NSToolbarItem, tool: &Tool) {
    let mtm = delegate.mtm();
    match tool {
        Tool::Search { value, .. } => {
            if let Some(item) = item.downcast_ref::<NSSearchToolbarItem>() {
                let field = item.searchField();
                if field.stringValue().to_string() != *value {
                    field.setStringValue(&NSString::from_str(value));
                }
            }
        }
        Tool::Button { label, glyph, enabled, .. } => {
            item.setLabel(&NSString::from_str(label));
            item.setToolTip(Some(&NSString::from_str(label)));
            item.setImage(symbol_image(glyph.sf(), label).as_deref());
            item.setEnabled(*enabled);
        }
        Tool::Choice { id, label, glyph, options, selected } => {
            item.setLabel(&NSString::from_str(label));
            item.setToolTip(Some(&NSString::from_str(label)));
            item.setImage(symbol_image(glyph.sf(), label).as_deref());
            if let Some(item) = item.downcast_ref::<NSMenuToolbarItem>() {
                let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(label));
                for (i, o) in options.iter().enumerate() {
                    // SAFETY: the selector names the delegate's method.
                    let m = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &NSString::from_str(o),
                            Some(sel!(toolChose:)),
                            ns_string!(""),
                        )
                    };
                    // SAFETY: as above.
                    unsafe { m.setTarget(Some(target(delegate))) };
                    m.setTag(i as isize);
                    m.setIdentifier(Some(&NSString::from_str(&format!("tool.{id}"))));
                    m.setState(if i == *selected { NSControlStateValueOn } else { NSControlStateValueOff });
                    menu.addItem(&m);
                }
                item.setMenu(&menu);
            }
        }
    }
}

/// Shows `tools` after the fixed items: items are made again when the set
/// changes (another page), and updated in place otherwise.
fn show_tools(toolbar: &NSToolbar, delegate: &Toolbar, tools: Vec<Tool>) {
    let wanted: Vec<String> = tools.iter().map(|t| format!("tool.{}", t.id())).collect();
    TOOLS.with(|t| *t.borrow_mut() = tools.clone());
    let items = toolbar.items();
    let have: Vec<String> = items.iter().skip(FIXED).map(|i| i.itemIdentifier().to_string()).collect();
    if have != wanted {
        for i in (FIXED..items.len()).rev() {
            toolbar.removeItemAtIndex(i as isize);
        }
        for (n, id) in wanted.iter().enumerate() {
            toolbar.insertItemWithItemIdentifier_atIndex(&NSString::from_str(id), (FIXED + n) as isize);
        }
    } else {
        for (item, tool) in items.iter().skip(FIXED).zip(&tools) {
            update_item(delegate, &item, tool);
        }
    }
}

// --- The window ---------------------------------------------------------------------

struct Installed {
    window: Retained<NSWindow>,
    sidebar: Retained<Sidebar>,
    toolbar: Retained<Toolbar>,
    footer: Footer,
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
        // The core's state at the sidebar's foot.
        let footer = Footer::new(mtm);
        let side = NSView::new(mtm);
        scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        footer.root.setTranslatesAutoresizingMaskIntoConstraints(false);
        side.addSubview(&scroll);
        side.addSubview(&footer.root);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            scroll.topAnchor().constraintEqualToAnchor(&side.topAnchor()),
            scroll.leadingAnchor().constraintEqualToAnchor(&side.leadingAnchor()),
            scroll.trailingAnchor().constraintEqualToAnchor(&side.trailingAnchor()),
            scroll.bottomAnchor().constraintEqualToAnchor_constant(&footer.root.topAnchor(), -6.0),
            footer.root.leadingAnchor().constraintEqualToAnchor_constant(&side.leadingAnchor(), 18.0),
            footer.root.trailingAnchor().constraintEqualToAnchor_constant(&side.trailingAnchor(), -14.0),
            footer.root.bottomAnchor().constraintEqualToAnchor_constant(&side.bottomAnchor(), -14.0),
        ]));
        let sidebar_vc = NSViewController::new(mtm);
        sidebar_vc.setView(&side);

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
            *i.borrow_mut() = Some(Installed { window: window.clone(), sidebar, toolbar: toolbar_delegate, footer, _split: split })
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
    let mut tools = chrome::bridge().toolbar.subscribe();
    std::thread::Builder::new()
        .name("kairo-chrome".into())
        .spawn(move || {
            let rt = kairo_core::tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
            rt.block_on(async move {
                loop {
                    let page = *pages.borrow_and_update();
                    let s = state.borrow_and_update().clone();
                    let l = live.borrow_and_update().clone();
                    let bar = tools.borrow_and_update().clone();
                    let theme = s.settings.theme;
                    dispatch2::DispatchQueue::main().exec_async(move || {
                        INSTALLED.with(|i| {
                            if let Some(i) = i.borrow().as_ref() {
                                set_appearance(&i.window, theme);
                                i.window.setTitle(&NSString::from_str(page.title()));
                                i.window.setSubtitle(&NSString::from_str(&bar.subtitle));
                                i.sidebar.select(page);
                                i.footer.update(&s, &l);
                                if let Some(toolbar) = i.window.toolbar() {
                                    show_tools(&toolbar, &i.toolbar, bar.tools);
                                }
                            }
                        })
                    });
                    kairo_core::tokio::select! {
                        r = pages.changed() => if r.is_err() { break },
                        r = state.changed() => if r.is_err() { break },
                        r = live.changed() => if r.is_err() { break },
                        r = tools.changed() => if r.is_err() { break },
                    }
                }
            });
        })
        .expect("chrome thread");
}

/// The sidebar's foot: the core's state, the mode and the speeds.
struct Footer {
    root: Retained<NSStackView>,
    dot: Retained<NSImageView>,
    status: Retained<NSTextField>,
    mode: Retained<NSTextField>,
    speeds: Retained<NSTextField>,
}

impl Footer {
    fn new(mtm: MainThreadMarker) -> Footer {
        let label = |size: f64, secondary: bool| {
            let l = NSTextField::labelWithString(ns_string!(""), mtm);
            l.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(size, unsafe { objc2_app_kit::NSFontWeightRegular })));
            if secondary {
                l.setTextColor(Some(&NSColor::secondaryLabelColor()));
            }
            l
        };
        let dot = NSImageView::new(mtm);
        dot.setImage(symbol_image("circle.fill", "").as_deref());
        dot.setSymbolConfiguration(Some(&objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(7.0, unsafe {
            objc2_app_kit::NSFontWeightRegular
        })));
        let status = label(12.0, false);
        status.setFont(Some(&NSFont::systemFontOfSize_weight(12.0, unsafe { objc2_app_kit::NSFontWeightMedium })));
        let mode = label(12.0, true);
        let top = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&[
            Retained::into_super(Retained::into_super(dot.clone())),
            Retained::into_super(Retained::into_super(status.clone())),
            Retained::into_super(Retained::into_super(mode.clone())),
        ]), mtm);
        top.setSpacing(6.0);
        let speeds = label(11.0, true);
        let root = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&[
            Retained::into_super(top),
            Retained::into_super(Retained::into_super(speeds.clone())),
        ]), mtm);
        root.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
        root.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
        root.setSpacing(3.0);
        Footer { root, dot, status, mode, speeds }
    }

    fn update(&self, s: &kairo_core::State, l: &kairo_core::Live) {
        self.status.setStringValue(&NSString::from_str(s.core.title()));
        self.mode.setStringValue(&NSString::from_str(&format!("· {}", s.settings.mode.title())));
        let tint = match &s.core {
            CoreStatus::Running { .. } => NSColor::systemGreenColor(),
            CoreStatus::Starting | CoreStatus::Stopping => NSColor::systemOrangeColor(),
            CoreStatus::Failed { .. } => NSColor::systemRedColor(),
            _ => NSColor::tertiaryLabelColor(),
        };
        self.dot.setContentTintColor(Some(&tint));
        let running = s.core.is_running();
        self.speeds.setHidden(!running);
        self.speeds.setStringValue(&NSString::from_str(&format!("↓ {}   ↑ {}", kairo_core::speed(l.down), kairo_core::speed(l.up))));
    }
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
