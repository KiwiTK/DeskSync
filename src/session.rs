use crate::config::Edge;
use crate::cursor;
use crate::proto::{delta_of, to_local, to_network, Frame, InputEvent};
use rdev::{Button, EventType, Key};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// 交出控制后本机光标停在离边缘多远的位置。
/// 光标贴边后操作系统不再给出位移，拉回一段距离才能继续读到移动量。
const NEUTRAL: f64 = 64.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Local,
    Forwarding { peer: String },
    Controlled { peer: String, entry: Edge },
}

pub struct Session {
    pub mode: Mode,
    pub w: f64,
    pub h: f64,
    /// 邻居名 -> 本机通向它的边。
    pub edges: HashMap<String, Edge>,
    /// 邻居名 -> 该连接的写通道。
    pub peers: HashMap<String, Sender<Frame>>,
    pub paused: bool,

    last_abs: Option<(f64, f64)>,
    edge_streak: u32,
    /// 被控时的虚拟光标位置。
    cur: (f64, f64),
    injected_pos: Option<(f64, f64)>,
    injected_at: Option<Instant>,
    injected_key: Option<(Key, Instant)>,
    injected_button: Option<(Button, Instant)>,
    pressed: HashSet<Key>,
    hidden: bool,
}

impl Session {
    pub fn new(w: f64, h: f64, edges: HashMap<String, Edge>) -> Self {
        Self {
            mode: Mode::Local,
            w,
            h,
            edges,
            peers: HashMap::new(),
            paused: false,
            last_abs: None,
            edge_streak: 0,
            cur: (w / 2.0, h / 2.0),
            injected_pos: None,
            injected_at: None,
            injected_key: None,
            injected_button: None,
            pressed: HashSet::new(),
            hidden: false,
        }
    }

    pub fn connect(&mut self, name: String, tx: Sender<Frame>) {
        self.peers.insert(name, tx);
    }

    pub fn disconnect(&mut self, name: &str) {
        self.peers.remove(name);
        let was_here = matches!(&self.mode, Mode::Forwarding { peer } if peer == name)
            || matches!(&self.mode, Mode::Controlled { peer, .. } if peer == name);
        if was_here {
            self.mode = Mode::Local;
            self.unhide();
        }
    }

    fn send(&self, peer: &str, frame: Frame) -> bool {
        match self.peers.get(peer) {
            Some(tx) => tx.send(frame).is_ok(),
            None => false,
        }
    }

    fn warp(&self, x: f64, y: f64) {
        let _ = rdev::simulate(&EventType::MouseMove {
            x: clamp(x, 1.0, self.w - 1.0),
            y: clamp(y, 1.0, self.h - 1.0),
        });
    }

    fn inject(&mut self, ev: &EventType) {
        let _ = rdev::simulate(ev);
        match ev {
            EventType::MouseMove { x, y } => {
                self.injected_pos = Some((*x, *y));
                self.injected_at = Some(Instant::now());
            }
            EventType::KeyPress(k) => self.injected_key = Some((*k, Instant::now())),
            EventType::ButtonPress(b) => self.injected_button = Some((*b, Instant::now())),
            _ => {}
        }
    }

    fn hide(&mut self) {
        if !self.hidden {
            cursor::hide();
            self.hidden = true;
        }
    }

    fn unhide(&mut self) {
        if self.hidden {
            cursor::show();
            self.hidden = false;
        }
    }

    /// 本机捕获到的输入。
    pub fn handle_local(&mut self, ev: &EventType) {
        match ev {
            EventType::KeyPress(k) => {
                self.pressed.insert(*k);
            }
            EventType::KeyRelease(k) => {
                self.pressed.remove(k);
            }
            _ => {}
        }
        if self.paused {
            return;
        }
        match self.mode.clone() {
            Mode::Local => self.idle(ev),
            Mode::Forwarding { peer } => self.forwarding(&peer, ev),
            Mode::Controlled { peer, entry } => self.controlled(&peer, entry, ev),
        }
    }

    fn idle(&mut self, ev: &EventType) {
        let EventType::MouseMove { x, y } = *ev else {
            return;
        };
        if let Some((peer, edge)) = self.edge_at(x, y) {
            self.edge_streak += 1;
            // 连续两次顶到同一条边才切换，避免路过边缘就跳走。
            if self.edge_streak >= 2 {
                self.edge_streak = 0;
                self.hand_off(&peer, edge, y / self.h);
                return;
            }
        } else {
            self.edge_streak = 0;
        }
        self.last_abs = Some((x, y));
    }

    fn edge_at(&self, x: f64, y: f64) -> Option<(String, Edge)> {
        let mut found = None;
        for (name, edge) in &self.edges {
            if !self.peers.contains_key(name) {
                continue;
            }
            let at = match edge {
                Edge::Left => x <= 1.0,
                Edge::Right => x >= self.w - 1.0,
                Edge::Top => y <= 1.0,
                Edge::Bottom => y >= self.h - 1.0,
            };
            if at {
                found = Some((name.clone(), *edge));
                break;
            }
        }
        found
    }

    fn hand_off(&mut self, peer: &str, edge: Edge, y_frac: f64) {
        if !self.peers.contains_key(peer) {
            return;
        }
        let x_frac = match edge {
            Edge::Left => 0.0,
            Edge::Right => 1.0,
            _ => clamp(self.last_abs.unwrap_or((self.w / 2.0, 0.0)).0 / self.w, 0.0, 1.0),
        };
        let y_frac = match edge {
            Edge::Top => 0.0,
            Edge::Bottom => 1.0,
            _ => clamp(y_frac, 0.0, 1.0),
        };
        if !self.send(
            peer,
            Frame::Enter {
                edge,
                x_frac,
                y_frac,
            },
        ) {
            return;
        }
        self.mode = Mode::Forwarding {
            peer: peer.to_string(),
        };
        self.last_abs = None;
        self.hide();
        let nx = match edge {
            Edge::Right => self.w - NEUTRAL,
            Edge::Left => NEUTRAL,
            _ => self.w / 2.0,
        };
        let ny = match edge {
            Edge::Bottom => self.h - NEUTRAL,
            Edge::Top => NEUTRAL,
            _ => y_frac * self.h,
        };
        self.warp(nx, ny);
    }

    fn forwarding(&mut self, peer: &str, ev: &EventType) {
        if let EventType::KeyPress(Key::Escape) = ev {
            let ctrl = self.pressed.contains(&Key::ControlLeft)
                || self.pressed.contains(&Key::ControlRight);
            let alt = self.pressed.contains(&Key::Alt);
            if ctrl && alt {
                self.reclaim(peer);
                return;
            }
        }
        let Some((net, newpos)) = to_network(ev, self.last_abs) else {
            return;
        };
        if !self.send(peer, Frame::Input(net)) {
            return;
        }
        if let EventType::MouseMove { x, y } = *ev {
            let stuck = x >= self.w - 1.0 || x <= 0.0 || y >= self.h - 1.0 || y <= 0.0;
            if stuck {
                let nx = if x >= self.w - 1.0 {
                    self.w - NEUTRAL
                } else if x <= 0.0 {
                    NEUTRAL
                } else {
                    x
                };
                let ny = if y >= self.h - 1.0 {
                    self.h - NEUTRAL
                } else if y <= 0.0 {
                    NEUTRAL
                } else {
                    y
                };
                self.warp(nx, ny);
                self.last_abs = None;
            } else {
                self.last_abs = Some(newpos);
            }
        }
    }

    /// 被控中：本机真实的键鼠活动会立刻收回控制。
    fn controlled(&mut self, peer: &str, _entry: Edge, ev: &EventType) {
        match ev {
            EventType::MouseMove { x, y } => {
                if let (Some((ix, iy)), Some(t)) = (self.injected_pos, self.injected_at) {
                    if (x - ix).abs() < 2.0 && (y - iy).abs() < 2.0 && t.elapsed() < Duration::from_millis(250)
                    {
                        return;
                    }
                }
                self.reclaim(peer);
            }
            EventType::KeyPress(k) => {
                if let Some((ik, t)) = self.injected_key {
                    if ik == *k && t.elapsed() < Duration::from_millis(120) {
                        return;
                    }
                }
                self.reclaim(peer);
            }
            EventType::ButtonPress(b) => {
                if let Some((ib, t)) = self.injected_button {
                    if ib == *b && t.elapsed() < Duration::from_millis(120) {
                        return;
                    }
                }
                self.reclaim(peer);
            }
            _ => {}
        }
    }

    fn reclaim(&mut self, peer: &str) {
        self.send(
            peer,
            Frame::Leave {
                x_frac: clamp(self.cur.0 / self.w, 0.0, 1.0),
                y_frac: clamp(self.cur.1 / self.h, 0.0, 1.0),
            },
        );
        self.mode = Mode::Local;
        self.unhide();
        self.last_abs = None;
        self.edge_streak = 0;
    }

    /// 邻居发来的报文。
    pub fn handle_frame(&mut self, peer: &str, frame: Frame) {
        match frame {
            Frame::Hello { .. } => {}
            Frame::Enter {
                edge,
                x_frac,
                y_frac,
            } => {
                let entry = edge.opposite();
                let (x, y) = self.edge_point(entry, x_frac, y_frac);
                self.cur = (x, y);
                self.mode = Mode::Controlled {
                    peer: peer.to_string(),
                    entry,
                };
                self.last_abs = None;
                self.inject(&EventType::MouseMove { x, y });
            }
            Frame::Leave { x_frac, y_frac } => {
                let mine = matches!(&self.mode, Mode::Forwarding { peer: p } if p == peer);
                if !mine {
                    return;
                }
                self.mode = Mode::Local;
                self.unhide();
                let edge = self.edges.get(peer).copied();
                let (x, y) = match edge {
                    Some(e) => self.edge_point(e, x_frac, y_frac),
                    None => (self.w / 2.0, self.h / 2.0),
                };
                self.warp(x, y);
                self.last_abs = None;
                self.edge_streak = 0;
            }
            Frame::Input(ev) => {
                let mine = matches!(&self.mode, Mode::Controlled { peer: p, .. } if p == peer);
                if !mine {
                    return;
                }
                self.apply_input(peer, ev);
            }
            Frame::Clipboard { .. } => {}
        }
    }

    fn apply_input(&mut self, peer: &str, ev: InputEvent) {
        let (dx, dy) = delta_of(&ev);
        if dx != 0.0 || dy != 0.0 {
            let nx = self.cur.0 + dx;
            let ny = self.cur.1 + dy;
            let entry = match self.mode {
                Mode::Controlled { entry, .. } => entry,
                _ => return,
            };
            let outside = match entry {
                Edge::Left => nx < 0.0,
                Edge::Right => nx > self.w - 1.0,
                Edge::Top => ny < 0.0,
                Edge::Bottom => ny > self.h - 1.0,
            };
            if outside {
                // 继续往回推：把控制还给对方。
                self.send(
                    peer,
                    Frame::Leave {
                        x_frac: clamp(nx / self.w, 0.0, 1.0),
                        y_frac: clamp(ny / self.h, 0.0, 1.0),
                    },
                );
                self.mode = Mode::Local;
                return;
            }
            self.cur = (clamp(nx, 0.0, self.w - 1.0), clamp(ny, 0.0, self.h - 1.0));
        }
        if let Some(local) = to_local(&ev, self.cur) {
            self.inject(&local);
        }
    }

    /// 在某条边上按归一化坐标算出像素位置。
    fn edge_point(&self, edge: Edge, x_frac: f64, y_frac: f64) -> (f64, f64) {
        match edge {
            Edge::Left => (1.0, clamp(y_frac, 0.0, 1.0) * (self.h - 1.0)),
            Edge::Right => (self.w - 1.0, clamp(y_frac, 0.0, 1.0) * (self.h - 1.0)),
            Edge::Top => (clamp(x_frac, 0.0, 1.0) * (self.w - 1.0), 1.0),
            Edge::Bottom => (clamp(x_frac, 0.0, 1.0) * (self.w - 1.0), self.h - 1.0),
        }
    }

    pub fn senders(&self) -> Vec<Sender<Frame>> {
        self.peers.values().cloned().collect()
    }
}

pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}
