//! Kairo's icons: one family of 24-point line glyphs (1.75 stroke, round caps,
//! in the manner of Lucide, ISC), drawn as SVG whose stroke is `currentColor`, so
//! they take the text color of where they are placed.

use nsh_core::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Overview,
    Proxies,
    Profiles,
    Connections,
    Rules,
    Logs,
    Dns,
    Settings,
    Bolt,
    Refresh,
    Plus,
    Close,
    Search,
    Play,
    Stop,
    Pause,
    Trash,
    Edit,
    Link,
    File,
    Check,
    ChevronRight,
    ChevronLeft,
    ChevronDown,
    ArrowUp,
    ArrowDown,
    Shield,
    Globe,
    Cpu,
    External,
    Sort,
    Restart,
    Download,
    Layers,
    Activity,
}

fn body(icon: Icon) -> &'static str {
    match icon {
        Icon::Activity => r#"<path d="M3 12h4l3-7 4 14 3-7h4"/>"#,
        Icon::Overview => {
            r#"<path d="M12 14l3.5-3.5"/><path d="M3.5 18a9 9 0 1 1 17 0"/><circle cx="12" cy="14" r="1.2"/>"#
        }
        Icon::Proxies => {
            r#"<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/><path d="M12 3a14 14 0 0 1 0 18a14 14 0 0 1 0-18z"/>"#
        }
        Icon::Profiles => {
            r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="M9 13h6M9 17h4"/>"#
        }
        Icon::Connections => {
            r#"<path d="M7 4v16"/><path d="M3.5 7.5L7 4l3.5 3.5"/><path d="M17 20V4"/><path d="M13.5 16.5L17 20l3.5-3.5"/>"#
        }
        Icon::Rules => r#"<path d="M4 6h16M7 12h10M10 18h4"/>"#,
        Icon::Logs => r#"<path d="M4 17l5-5-5-5"/><path d="M12 19h8"/>"#,
        Icon::Dns => {
            r#"<rect x="3" y="4" width="18" height="7" rx="2"/><rect x="3" y="13" width="18" height="7" rx="2"/><path d="M7 7.5h.01M7 16.5h.01"/>"#
        }
        Icon::Settings => {
            r#"<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>"#
        }
        Icon::Bolt => r#"<path d="M13 2L4 14h7l-1 8 9-12h-7z"/>"#,
        Icon::Refresh => {
            r#"<path d="M20 11a8 8 0 0 0-14.8-4.2L3 9"/><path d="M3 4v5h5"/><path d="M4 13a8 8 0 0 0 14.8 4.2L21 15"/><path d="M21 20v-5h-5"/>"#
        }
        Icon::Plus => r#"<path d="M12 5v14M5 12h14"/>"#,
        Icon::Close => r#"<path d="M6 6l12 12M18 6L6 18"/>"#,
        Icon::Search => r#"<circle cx="11" cy="11" r="7"/><path d="M20 20l-4-4"/>"#,
        Icon::Play => r#"<path d="M7 4.5v15l12-7.5z"/>"#,
        Icon::Stop => r#"<rect x="6" y="6" width="12" height="12" rx="1.5"/>"#,
        Icon::Pause => r#"<path d="M9 5v14M15 5v14"/>"#,
        Icon::Trash => {
            r#"<path d="M4 7h16"/><path d="M10 11v6M14 11v6"/><path d="M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12"/><path d="M9 7V4h6v3"/>"#
        }
        Icon::Edit => r#"<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>"#,
        Icon::Link => {
            r#"<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>"#
        }
        Icon::File => {
            r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"#
        }
        Icon::Check => r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#,
        Icon::ChevronRight => r#"<path d="M9 6l6 6-6 6"/>"#,
        Icon::ChevronLeft => r#"<path d="M15 6l-6 6 6 6"/>"#,
        Icon::ChevronDown => r#"<path d="M6 9l6 6 6-6"/>"#,
        Icon::ArrowUp => r#"<path d="M12 19V5M6 11l6-6 6 6"/>"#,
        Icon::ArrowDown => r#"<path d="M12 5v14M6 13l6 6 6-6"/>"#,
        Icon::Shield => r#"<path d="M12 3l8 3v6c0 4.5-3.4 8.2-8 9-4.6-.8-8-4.5-8-9V6z"/>"#,
        Icon::Globe => {
            r#"<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c2.5 2.7 3.8 5.7 3.8 9s-1.3 6.3-3.8 9c-2.5-2.7-3.8-5.7-3.8-9S9.5 5.7 12 3z"/>"#
        }
        Icon::Cpu => {
            r#"<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M10 2v4M14 2v4M10 18v4M14 18v4M2 10h4M2 14h4M18 10h4M18 14h4"/>"#
        }
        Icon::External => {
            r#"<path d="M14 4h6v6"/><path d="M20 4l-9 9"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>"#
        }
        Icon::Sort => {
            r#"<path d="M7 4v16M3.5 16.5L7 20l3.5-3.5"/><path d="M14 6h7M14 12h5M14 18h3"/>"#
        }
        Icon::Restart => r#"<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5"/>"#,
        Icon::Download => r#"<path d="M12 4v11"/><path d="M7 10l5 5 5-5"/><path d="M5 20h14"/>"#,
        Icon::Layers => r#"<path d="M12 3l9 5-9 5-9-5z"/><path d="M3 13l9 5 9-5"/>"#,
    }
}

/// The data URI of an icon.
pub fn uri(icon: Icon) -> String {
    format!(
        "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.75' stroke-linecap='round' stroke-linejoin='round'>{}</svg>",
        body(icon)
    )
}

/// An icon `size` points square in the surrounding text color.
pub fn icon(icon: Icon, size: f32) -> Svg {
    svg(uri(icon)).size(px(size)).flex_shrink_0()
}
