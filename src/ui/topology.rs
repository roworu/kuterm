//! pane topology: panes as nodes on a circle with lines between those whose programs talk
//! to each other, found by matching the local tcp connections and unix socket pairs of each
//! pane's process tree. a line pulses while its ends move bytes, a click on a node goes to
//! its pane

use std::{collections::HashMap, time::Duration};

use gpui::{
    App, Bounds, Context, DismissEvent, EntityId, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, PathBuilder, Pixels, Point, Task, Window, canvas, div, point, prelude::*, px,
    relative,
};

use crate::{
    terminal::{ProcessSampler, TreeSample},
    theme::Theme,
};

// numbers come once a second, so the map does too
const REFRESH: Duration = Duration::from_secs(1);
// nodes sit on a circle this far from the center, as a share of the map
const RADIUS: f32 = 0.35;
// nodes are boxes this wide and tall, centered on their place
const NODE_WIDTH: f32 = 140.;
const NODE_HEIGHT: f32 = 26.;

/// how two panes talk
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wire {
    /// a program of one connects to this port another listens on
    Tcp(u16),
    /// a unix socket pair, like a database client and its server
    Unix,
}

/// panes `from` and `to` talking over `wire`, indices into the nodes
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub wire: Wire,
}

/// edges between the trees of `samples`: a connection from one to a port another listens
/// on, and unix sockets whose peer the other holds. each pair and wire once
pub fn edges(samples: &[&TreeSample]) -> Vec<Edge> {
    let mut found = Vec::new();
    // which tree holds which unix socket
    let holders: HashMap<u64, usize> = samples
        .iter()
        .enumerate()
        .flat_map(|(ix, sample)| {
            sample
                .unix_sockets
                .iter()
                .map(move |(inode, _)| (*inode, ix))
        })
        .collect();
    for (from, sample) in samples.iter().enumerate() {
        for (to, other) in samples.iter().enumerate() {
            if from == to {
                continue;
            }
            for port in &sample.connected_ports {
                // a tree talking to its own server is no edge, the listener must be the other
                if other.ports.contains(port) && !sample.ports.contains(port) {
                    found.push(Edge {
                        from,
                        to,
                        wire: Wire::Tcp(*port),
                    });
                }
            }
        }
        for (_, peer) in &sample.unix_sockets {
            if let Some(&to) = holders.get(peer)
                && to > from
            {
                found.push(Edge {
                    from,
                    to,
                    wire: Wire::Unix,
                });
            }
        }
    }
    found.sort_by_key(|edge| (edge.from, edge.to));
    found.dedup();
    found
}

/// where node `ix` of `count` sits, as fractions of the map's width and height
pub fn node_center(ix: usize, count: usize) -> (f32, f32) {
    if count <= 1 {
        return (0.5, 0.5);
    }
    let angle = std::f32::consts::TAU * ix as f32 / count as f32 - std::f32::consts::FRAC_PI_2;
    (0.5 + RADIUS * angle.cos(), 0.5 + RADIUS * angle.sin())
}

/// a pane on the map
pub struct TopologyNode {
    pub id: EntityId,
    pub title: String,
    pub shell_pid: u32,
}

/// the node picked from the map
pub struct TopologyPicked(pub EntityId);

pub struct Topology {
    nodes: Vec<TopologyNode>,
    samples: Vec<TreeSample>,
    /// bytes each node had moved on the refresh before, to tell which ones move now
    last_io: Vec<u64>,
    busy: Vec<bool>,
    focus: FocusHandle,
    _refresh: Task<()>,
}

impl EventEmitter<DismissEvent> for Topology {}
impl EventEmitter<TopologyPicked> for Topology {}

impl Topology {
    /// the map of `nodes`, refreshing itself every second
    pub fn new(nodes: Vec<TopologyNode>, cx: &mut Context<Self>) -> Self {
        let count = nodes.len();
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(REFRESH).await;
            }
        });
        Self {
            nodes,
            samples: vec![TreeSample::default(); count],
            last_io: vec![0; count],
            busy: vec![false; count],
            focus: cx.focus_handle(),
            _refresh: refresh,
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        for (ix, node) in self.nodes.iter().enumerate() {
            // asking keeps the sampler running while the map is open
            if let Some(sample) = ProcessSampler::shared().sample(node.shell_pid) {
                self.busy[ix] = self.last_io[ix] != 0 && sample.io_bytes != self.last_io[ix];
                self.last_io[ix] = sample.io_bytes;
                self.samples[ix] = sample;
            }
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            cx.emit(DismissEvent);
            cx.stop_propagation();
        }
    }
}

impl Focusable for Topology {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Topology {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let samples: Vec<&TreeSample> = self.samples.iter().collect();
        let edges = edges(&samples);
        let labels = edge_labels(&edges, &self.nodes);
        let count = self.nodes.len();
        let busy = self.busy.clone();
        let (quiet, pulsing) = (theme.text_muted, theme.attention);
        let lines = canvas(
            |_, _, _| {},
            move |bounds: Bounds<Pixels>, _, window, _| {
                let at = |ix: usize| -> Point<Pixels> {
                    let (x, y) = node_center(ix, count);
                    bounds.origin + point(bounds.size.width * x, bounds.size.height * y)
                };
                for edge in &edges {
                    let pulse = busy[edge.from] || busy[edge.to];
                    let mut path = PathBuilder::stroke(if pulse { px(3.) } else { px(1.5) });
                    path.move_to(at(edge.from));
                    path.line_to(at(edge.to));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, if pulse { pulsing } else { quiet });
                    }
                }
            },
        )
        .absolute()
        .size_full();
        let nodes = self.nodes.iter().enumerate().map(|(ix, node)| {
            let (x, y) = node_center(ix, count);
            let id = node.id;
            div()
                .id(("topology-node", ix))
                .debug_selector(move || format!("topology-node-{ix}"))
                .absolute()
                .left(relative(x))
                .top(relative(y))
                .ml(px(-NODE_WIDTH / 2.))
                .mt(px(-NODE_HEIGHT / 2.))
                .w(px(NODE_WIDTH))
                .h(px(NODE_HEIGHT))
                .px_2()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.tab_bar_background)
                .cursor_pointer()
                .hover(|node| node.bg(theme.tab_active_background))
                .child(div().truncate().child(node.title.clone()))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(TopologyPicked(id))))
        });
        let legend = if labels.is_empty() {
            "no panes talk to each other, escape closes".to_string()
        } else {
            format!("{}, escape closes", labels.join(" · "))
        };
        div()
            .id("topology")
            .debug_selector(|| "topology".into())
            .key_context("Topology")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .absolute()
            .inset_0()
            .bg(theme.terminal_background.opacity(0.95))
            .text_color(theme.text)
            .child(lines)
            .children(nodes)
            .child(
                div()
                    .debug_selector(|| "topology-legend".into())
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .p_2()
                    .text_sm()
                    .text_color(theme.text_muted)
                    .child(legend),
            )
    }
}

/// "build → web :8080" for each edge
fn edge_labels(edges: &[Edge], nodes: &[TopologyNode]) -> Vec<String> {
    edges
        .iter()
        .filter_map(|edge| {
            let (from, to) = (nodes.get(edge.from)?, nodes.get(edge.to)?);
            Some(match edge.wire {
                Wire::Tcp(port) => format!("{} → {} :{port}", from.title, to.title),
                Wire::Unix => format!("{} ↔ {} (unix)", from.title, to.title),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Edge, Wire, edges, node_center};
    use crate::terminal::TreeSample;

    #[test]
    fn edges_come_from_ports_and_socket_pairs() {
        let server = TreeSample {
            ports: vec![8080],
            unix_sockets: vec![(10, 20)],
            ..TreeSample::default()
        };
        let tests = TreeSample {
            connected_ports: vec![8080, 443],
            ..TreeSample::default()
        };
        let database = TreeSample {
            unix_sockets: vec![(20, 10)],
            ..TreeSample::default()
        };
        // talks to itself only
        let lonely = TreeSample {
            ports: vec![9000],
            connected_ports: vec![9000],
            ..TreeSample::default()
        };
        let found = edges(&[&server, &tests, &database, &lonely]);
        assert_eq!(
            found,
            [
                Edge {
                    from: 0,
                    to: 2,
                    wire: Wire::Unix
                },
                Edge {
                    from: 1,
                    to: 0,
                    wire: Wire::Tcp(8080)
                },
            ]
        );
        assert!(edges(&[]).is_empty());
    }

    #[test]
    fn nodes_sit_on_a_circle() {
        assert_eq!(node_center(0, 1), (0.5, 0.5));
        // the first on top, the next to the right
        let (x, y) = node_center(0, 4);
        assert!((x - 0.5).abs() < 0.001 && y < 0.5);
        let (x, y) = node_center(1, 4);
        assert!(x > 0.5 && (y - 0.5).abs() < 0.001);
    }
}
