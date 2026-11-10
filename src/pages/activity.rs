//! Activity: which tools talk to which AI services, drawn as a flow. Tools on
//! the left, the exit in the middle, services on the right; a line is as thick
//! as the bytes it carried and dashed when it stays off the exit (domestic AI
//! and local models). Nothing here reads a URL or a key: the core reports
//! which process opened a connection to which host, and that is all.

use kairo_core::activity::{Flow, Tally, Target};
use kairo_core::ai;
use kairo_core::bytes;
use nsh_base::theme;
use nsh_core::canvas::LineCap;
use nsh_core::prelude::*;

use crate::pages::ai as board;
use crate::store::store;
use crate::ui::toolbar;
use crate::ui::{self, Icon, size};

/// Height of one row of tools or services.
const ROW: f32 = 50.0;
/// Width of the exit in the middle.
const HUB: f32 = 156.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub tool: usize,
    pub target: usize,
    pub bytes: u64,
    pub open: u32,
    /// Many of its connections ended at once, moving almost nothing.
    pub shaky: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub title: String,
    pub detail: String,
    /// Stays off the exit.
    pub direct: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Graph {
    pub tools: Vec<Node>,
    /// Services through the exit first, then the direct ones.
    pub targets: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn rows(&self) -> usize {
        self.tools.len().max(self.targets.len())
    }

    /// The centre (in rows) of the targets that go through the exit and of
    /// the direct ones.
    pub fn hubs(&self) -> (Option<f32>, Option<f32>) {
        let via = self.targets.iter().filter(|t| !t.direct).count();
        let direct = self.targets.len() - via;
        (
            (via > 0).then(|| via as f32 / 2.0),
            (direct > 0).then(|| via as f32 + direct as f32 / 2.0),
        )
    }
}

fn target_title(t: &Target) -> (String, bool) {
    match t {
        Target::Service(id) => (
            ai::SERVICES
                .iter()
                .find(|s| s.id == *id)
                .map(|s| s.name.to_owned())
                .unwrap_or_else(|| (*id).to_owned()),
            false,
        ),
        Target::Domestic(n) => ((*n).to_owned(), true),
        Target::Local => ("On this computer".to_owned(), true),
    }
}

fn target_order(t: &Target) -> (u8, usize) {
    match t {
        Target::Service(id) => (
            0,
            ai::SERVICES.iter().position(|s| s.id == *id).unwrap_or(99),
        ),
        Target::Domestic(_) => (1, 0),
        Target::Local => (2, 0),
    }
}

fn flow_detail(f: &Flow, shaky: bool) -> String {
    let mut parts = Vec::new();
    if f.open > 0 {
        parts.push(format!("{} open", f.open));
    }
    if f.up + f.down > 0 {
        parts.push(bytes(f.up + f.down));
    }
    if shaky {
        parts.push(format!("{} ended at once", f.short));
    }
    if parts.is_empty() {
        "Idle".into()
    } else {
        parts.join(" · ")
    }
}

/// Sorts the tally into the graph.
pub fn graph(tally: &Tally) -> Graph {
    let flows = tally.flows();
    let mut tools: Vec<(String, u64, u32)> = Vec::new();
    let mut targets: Vec<Target> = Vec::new();
    for (tool, target, f) in &flows {
        match tools.iter_mut().find(|t| t.0 == *tool) {
            Some(t) => {
                t.1 += f.up + f.down;
                t.2 += f.open;
            }
            None => tools.push(((*tool).to_owned(), f.up + f.down, f.open)),
        }
        if !targets.contains(target) {
            targets.push((*target).clone());
        }
    }
    tools.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    targets.sort_by_key(target_order);
    let edges: Vec<Edge> = flows
        .iter()
        .map(|(tool, target, f)| Edge {
            tool: tools.iter().position(|t| t.0 == *tool).unwrap_or(0),
            target: targets.iter().position(|t| t == *target).unwrap_or(0),
            bytes: f.up + f.down,
            open: f.open,
            shaky: f.seen >= 3 && f.short * 2 >= f.seen,
        })
        .collect();
    let tool_nodes = tools
        .iter()
        .enumerate()
        .map(|(i, (name, total, open))| {
            let direct = edges.iter().filter(|e| e.tool == i).all(|e| {
                targets[e.target] != Target::Service("") && target_title(&targets[e.target]).1
            });
            Node {
                title: name.clone(),
                detail: match (*open, *total) {
                    (0, 0) => "Idle".into(),
                    (0, t) => bytes(t),
                    (o, 0) => format!("{o} open"),
                    (o, t) => format!("{o} open · {}", bytes(t)),
                },
                direct,
            }
        })
        .collect();
    let target_nodes = targets
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (title, direct) = target_title(t);
            let mut total = Flow::default();
            for e in edges.iter().filter(|e| e.target == i) {
                total.open += e.open;
                total.up += e.bytes;
            }
            let shaky = edges.iter().any(|e| e.target == i && e.shaky);
            let short: u32 = flows
                .iter()
                .filter(|(_, tg, _)| *tg == t)
                .map(|(_, _, f)| f.short)
                .sum();
            total.short = short;
            Node {
                title,
                detail: flow_detail(&total, shaky),
                direct,
            }
        })
        .collect();
    Graph {
        tools: tool_nodes,
        targets: target_nodes,
        edges,
    }
}

fn weight(bytes: u64, max: u64) -> f32 {
    if max == 0 {
        return 1.5;
    }
    1.5 + 5.0 * (bytes as f32 / max as f32).sqrt()
}

pub fn page() -> impl IntoElement {
    let store = store();
    let graph = computed(move || store.activity.with(|a| graph(&a.0)));
    let empty = computed(move || graph.with(|g| g.edges.is_empty()));
    view()
        .flex_1()
        .child(toolbar::floating(
            crate::store::Page::Activity,
            String::new,
            vec![],
        ))
        .child(dynamic(move || {
            // Only a change between "nothing" and "something" remounts the
            // page; the numbers moving do not (the scroll position stays).
            if empty.get() {
                untracked(nothing).into_element()
            } else {
                untracked(move || flow(graph)).into_element()
            }
        }))
}

fn nothing() -> impl IntoElement {
    ui::empty_state(
        Icon::Activity,
        "No AI traffic yet",
        "Open Claude Code, Cursor or any AI tool. It shows up here, with the service it talks to and the way it goes.",
        None,
    )
}

fn flow(graph: Memo<Graph>) -> impl IntoElement {
    let store = store();
    let width = signal(0.0_f32);
    scroll().flex_1().child(
        view()
            .px_8()
            .pt(toolbar::scroll_top(28.0))
            .pb_8()
            .gap_5()
            .w_full()
            .max_w(px(860.0))
            .self_center()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_1()
                    .child(
                        view()
                            .flex_1()
                            .child(ui::title("Activity"))
                            .child(ui::muted(
                                "Which tools talk to which AI services, since Kairo opened.",
                            )),
                    )
                    .child(ui::link_btn("Clear").on_press(move || store.svc().clear_activity())),
            )
            .child(dynamic(move || {
                let g = graph.get();
                untracked(move || figure(g, width).into_element())
            }))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_1()
                    .child(
                        ui::link_btn("All connections")
                            .on_press(move || {
                                store.all_connections.set(true);
                                store.page.set(crate::store::Page::Activity)
                            }),
                    )
                    .child(view().flex_1())
                    .child(
                        ui::muted(
                            "Counts only: tool, service and bytes. No addresses, no content.",
                        )
                        .text_size(px(size::small())),
                    ),
            ),
    )
}

fn column(nodes: Vec<Node>, right: bool) -> impl IntoElement {
    view()
        .w(px(200.0))
        .flex_shrink_0()
        .children(nodes.into_iter().map(move |n| {
            view()
                .h(px(ROW))
                .justify_center()
                .items(if right { Align::Start } else { Align::End })
                .child(
                    ui::body(n.title)
                        .font_weight(FontWeight::SEMIBOLD)
                        .number_of_lines(1),
                )
                .child(
                    ui::muted(n.detail)
                        .text_size(px(size::small()))
                        .number_of_lines(1),
                )
        }))
}

trait AlignExt {
    fn items(self, a: Align) -> Self;
}

impl AlignExt for View {
    fn items(self, a: Align) -> Self {
        match a {
            Align::Start => self.items_start(),
            Align::End => self.items_end(),
        }
    }
}

#[derive(Clone, Copy)]
enum Align {
    Start,
    End,
}

fn figure(g: Graph, width: Signal<f32>) -> impl IntoElement {
    let rows = g.rows().max(1);
    let height = rows as f32 * ROW;
    let (via, direct) = g.hubs();
    let max = g.edges.iter().map(|e| e.bytes).max().unwrap_or(0);
    let tools = g.tools.clone();
    let targets = g.targets.clone();
    let tool_y = move |i: usize| (i as f32 + 0.5) * ROW;
    let target_y = move |i: usize| (i as f32 + 0.5) * ROW;
    // Two canvases (tools to exit, exit to services); both are half of what
    // is left between the columns and the exit.
    let side = {
        let g = g.clone();
        move |left: bool| {
            let g = g.clone();
            canvas(move || {
                let w = width.get() / 2.0;
                let mut p = Painter::new();
                if w <= 0.0 {
                    return p;
                }
                let accent = color(theme().primary.opacity(0.6));
                let quiet = color(theme().muted_foreground.opacity(0.35));
                let warn = color(ui::warning());
                let hub = |direct: bool| -> f32 {
                    let r = if direct { direct_row(&g) } else { via_row(&g) };
                    r * ROW
                };
                for e in &g.edges {
                    let tgt = &g.targets[e.target];
                    let hub_y = hub(tgt.direct);
                    let (y0, y1) = if left {
                        (tool_y(e.tool), hub_y)
                    } else {
                        (hub_y, target_y(e.target))
                    };
                    let stroke = if e.shaky {
                        warn
                    } else if e.open == 0 {
                        quiet
                    } else if tgt.direct {
                        color(theme().muted_foreground.opacity(0.6))
                    } else {
                        accent
                    };
                    let dash: Vec<Points> = if tgt.direct {
                        vec![px(2.0), px(7.0)]
                    } else {
                        vec![]
                    };
                    let (x0, x1) = (0.0, w);
                    let mid = (x0 + x1) / 2.0;
                    p = p
                        .begin_path()
                        .move_to(x0, y0)
                        .bezier_curve_to(
                            point(px(mid), px(y0)),
                            point(px(mid), px(y1)),
                            point(px(x1), px(y1)),
                        )
                        .stroke_color(stroke)
                        .line_width(weight(e.bytes, max))
                        .line_cap(LineCap::Round)
                        .line_dash(&dash)
                        .stroke();
                }
                p
            })
            .h(px(height))
            .flex_1()
            .on_layout(move |frame: &Bounds<Points>| {
                if left {
                    width.set(frame.size.width.0 * 2.0);
                }
            })
        }
    };
    let exit_hub = via.map(|y| hub_node(y, true));
    let direct_hub = direct.map(|y| hub_node(y, false));
    view()
        .flex_row()
        .items_start()
        .child(column(tools, false))
        .child(side(true))
        .child(
            view()
                .w(px(HUB))
                .h(px(height))
                .flex_shrink_0()
                .relative()
                .child(exit_hub)
                .child(direct_hub),
        )
        .child(side(false))
        .child(column(targets, true))
}

fn via_row(g: &Graph) -> f32 {
    g.hubs().0.unwrap_or(0.0)
}

fn direct_row(g: &Graph) -> f32 {
    g.hubs().1.unwrap_or(0.0)
}

/// The exit (or "Direct") in the middle, centred on `row`.
fn hub_node(row: f32, exit: bool) -> impl IntoElement {
    let top = (row * ROW - 24.0).max(0.0);
    let label = move || -> SharedString {
        if !exit {
            return "Direct".into();
        }
        match board::exit_place() {
            Some(p) => format!("{}{}", p.flag.map(|f| f + " ").unwrap_or_default(), p.title).into(),
            None => "Your Location".into(),
        }
    };
    view()
        .absolute()
        .top(px(top))
        .left_0()
        .right_0()
        .h(px(48.0))
        .items_center()
        .justify_center()
        .rounded(px(18.0))
        .bg(move || theme().muted)
        .px_2()
        .child(ui::muted(if exit { "AI exit" } else { "Not proxied" }).text_size(px(size::small())))
        .child(
            ui::body(label)
                .font_weight(FontWeight::SEMIBOLD)
                .number_of_lines(1),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kairo_core::activity::parse_time;
    use kairo_core::model::Connection;

    fn conn(
        id: &str,
        host: &str,
        port: &str,
        process: &str,
        path: &str,
        up: u64,
        down: u64,
    ) -> Connection {
        Connection {
            id: id.into(),
            host: host.into(),
            port: port.into(),
            network: "tcp".into(),
            inbound: String::new(),
            process: process.into(),
            process_path: path.into(),
            source: String::new(),
            destination_ip: String::new(),
            rule: String::new(),
            rule_payload: String::new(),
            chains: vec![],
            upload: up,
            download: down,
            up_speed: 0,
            down_speed: 0,
            start: "2026-11-09T15:00:00Z".into(),
        }
    }

    fn tally() -> Tally {
        let mut t = Tally::default();
        let now = parse_time("2026-11-09T15:00:10Z").unwrap();
        t.ingest(
            &[
                conn(
                    "1",
                    "api.anthropic.com",
                    "443",
                    "claude",
                    "/usr/bin/claude",
                    10_000,
                    900_000,
                ),
                conn(
                    "2",
                    "api.openai.com",
                    "443",
                    "codex",
                    "/usr/bin/codex",
                    1_000,
                    40_000,
                ),
                conn(
                    "3",
                    "api.deepseek.com",
                    "443",
                    "Cursor",
                    "/Applications/Cursor.app/Contents/MacOS/Cursor",
                    500,
                    5_000,
                ),
                conn("4", "127.0.0.1", "11434", "ollama", "/usr/bin/ollama", 5, 5),
                conn(
                    "5",
                    "api.anthropic.com",
                    "443",
                    "Cursor",
                    "/Applications/Cursor.app/Contents/MacOS/Cursor",
                    500,
                    5_000,
                ),
            ],
            now,
        );
        t
    }

    #[test]
    fn tools_and_services_are_sorted_into_a_graph() {
        let g = graph(&tally());
        // Most traffic first.
        assert_eq!(g.tools[0].title, "Claude");
        let names: Vec<&str> = g.targets.iter().map(|t| t.title.as_str()).collect();
        // Through the exit first, in the order the board shows them; then
        // the domestic ones; then what stays on this computer.
        assert_eq!(names, ["Claude", "ChatGPT", "DeepSeek", "On this computer"]);
        assert_eq!(g.targets.iter().filter(|t| t.direct).count(), 2);
        assert_eq!(g.edges.len(), 5);
    }

    #[test]
    fn hubs_sit_in_the_middle_of_their_targets() {
        let g = graph(&tally());
        let (via, direct) = g.hubs();
        assert_eq!(via, Some(1.0));
        assert_eq!(direct, Some(3.0));
        assert_eq!(Graph::default().hubs(), (None, None));
    }

    #[test]
    fn a_line_is_thicker_for_more_bytes() {
        assert!(weight(900_000, 900_000) > weight(40_000, 900_000));
        assert!(weight(0, 900_000) >= 1.5);
        assert_eq!(weight(0, 0), 1.5);
    }

    #[test]
    fn connections_that_end_at_once_are_marked() {
        let mut t = Tally::default();
        let start = parse_time("2026-11-09T15:00:00Z").unwrap();
        for i in 0..4 {
            let id = format!("c{i}");
            let c = conn(
                &id,
                "api.openai.com",
                "443",
                "codex",
                "/usr/bin/codex",
                0,
                0,
            );
            t.ingest(&[c], start + i * 2);
            t.ingest(&[], start + i * 2 + 1);
        }
        let g = graph(&t);
        assert!(g.edges[0].shaky);
        assert!(
            g.targets[0].detail.contains("ended at once"),
            "{}",
            g.targets[0].detail
        );
    }
}
