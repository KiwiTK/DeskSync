use crate::config::{Edge, Rect};
use crate::cursor;
use crate::proto::{delta_of, to_local, to_network, Frame, InputEvent};
use rdev::{Button, EventType, Key};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// 交出控制后本机光标停在离边缘多远的位置。
/// 光标贴边后操作系统不再给出位移，拉回一段距离才能继续读到移动量。
const NEUTRAL: f64 = 64.0;

/// 摆放容差：两条屏幕边允许有几像素的缝隙或交叠仍算相接。
const TOL: i64 = 4;

/// 光标贴边判定距离（像素）。
const MARGIN: f64 = 1.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Local,
    Forwarding { peer: String },
    /// 被控中。`exit` 是本机通向控制方的那条边（光标往那边推就归还控制）。
    Controlled { peer: String, exit: Edge },
}

pub struct Session {
    pub mode: Mode,
    pub w: f64,
    pub h: f64,
    /// 虚拟桌面原点：Windows 多显示器下鼠标坐标以主屏左上为 0,0，副屏在左边/上边时为负。
    /// 捕获进来的坐标已经减掉它，注入时要加回去。
    ox: f64,
    oy: f64,
    pub self_name: String,
    /// 每台机器在虚拟画布上的矩形（含本机）。邻居的尺寸会被 Hello 校正成真实分辨率。
    pub layout: HashMap<String, Rect>,
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
    pub fn new(w: f64, h: f64, self_name: String, layout: HashMap<String, Rect>) -> Self {
        Self {
            mode: Mode::Local,
            w,
            h,
            ox: 0.0,
            oy: 0.0,
            self_name,
            layout,
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

    /// Windows 多显示器：告诉会话鼠标坐标系的原点偏移，注入时加回来。
    /// 不调用就是 0,0，行为跟以前一致。
    pub fn set_origin(&mut self, ox: f64, oy: f64) {
        self.ox = ox;
        self.oy = oy;
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

    /// UI 在配置文件改完后调用：新的布局直接生效，不用重启。
    pub fn set_layout(&mut self, layout: HashMap<String, Rect>) {
        self.layout = layout;
        self.edge_streak = 0;
    }

    /// 握手后校正邻居矩形的尺寸为对方真实分辨率（位置不变）。
    /// 用户在画布里摆位置时不用关心对方屏幕多大。
    pub fn update_peer_screen(&mut self, name: &str, screen: (u32, u32)) {
        if name == &self.self_name {
            return;
        }
        let me = self.self_rect();
        let entry = self
            .layout
            .entry(name.to_string())
            .or_insert_with(|| Rect::new(me.right(), me.y, screen.0, screen.1));
        if entry.w != screen.0 || entry.h != screen.1 {
            crate::log::write(&format!(
                "布局: {} 的矩形尺寸校正为 {}x{}（Hello 汇报的真实分辨率）",
                name, screen.0, screen.1
            ));
            entry.w = screen.0;
            entry.h = screen.1;
        }
    }

    fn self_rect(&self) -> Rect {
        self.layout.get(&self.self_name).copied().unwrap_or(Rect::new(
            0,
            0,
            self.w as u32,
            self.h as u32,
        ))
    }

    /// 把控制权从当前被控制的机器上收回来。本机不是主导方时返回 false。
    pub fn force_return(&mut self) -> bool {
        if let Mode::Forwarding { peer } = self.mode.clone() {
            self.reclaim(&peer);
            true
        } else {
            false
        }
    }

    /// 给 UI 看的状态标签。
    pub fn mode_str(&self) -> String {
        match &self.mode {
            Mode::Local => "local".to_string(),
            Mode::Forwarding { peer } => format!("forwarding:{peer}"),
            Mode::Controlled { peer, .. } => format!("controlled:{peer}"),
        }
    }

    fn send(&self, peer: &str, frame: Frame) -> bool {
        match self.peers.get(peer) {
            Some(tx) => tx.send(frame).is_ok(),
            None => false,
        }
    }

    fn warp(&self, x: f64, y: f64) {
        // 会话里的坐标是「减过原点」的，注入要还原成系统坐标。
        let _ = rdev::simulate(&EventType::MouseMove {
            x: clamp(x, 1.0, self.w - 1.0) + self.ox,
            y: clamp(y, 1.0, self.h - 1.0) + self.oy,
        });
    }

    fn inject(&mut self, ev: &EventType) {
        match ev {
            EventType::MouseMove { x, y } => {
                let _ = rdev::simulate(&EventType::MouseMove {
                    x: *x + self.ox,
                    y: *y + self.oy,
                });
                // 记下来的是会话坐标，后面跟捕获到的坐标（同样减过原点）比。
                self.injected_pos = Some((*x, *y));
                self.injected_at = Some(Instant::now());
            }
            other => {
                let _ = rdev::simulate(other);
            }
        }
        match ev {
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
            Mode::Controlled { peer, exit } => self.controlled(&peer, exit, ev),
        }
    }

    fn idle(&mut self, ev: &EventType) {
        let EventType::MouseMove { x, y } = *ev else {
            return;
        };
        if let Some((peer, vx, vy)) = self.exit_target(x, y) {
            self.edge_streak += 1;
            // 连续两次顶到边界才切换，避免路过边缘就跳走。
            if self.edge_streak >= 2 {
                self.edge_streak = 0;
                self.hand_off(&peer, vx, vy, x, y);
                return;
            }
        } else {
            self.edge_streak = 0;
        }
        self.last_abs = Some((x, y));
    }

    /// 本机光标 (x, y)（屏幕坐标）是否顶到了与某邻居相接的边界。
    /// 返回 (邻居, 虚拟画布上的进入点)。相邻判定是几何的：
    /// 只要对方的矩形与我这条边有重叠段、且光标落在重叠段内就成立，
    /// 所以同一侧可以并排多台屏幕、也可以摆出任意错位的组合。
    fn exit_target(&self, x: f64, y: f64) -> Option<(String, f64, f64)> {
        let at_left = x <= MARGIN;
        let at_right = x >= self.w - MARGIN;
        let at_top = y <= MARGIN;
        let at_bottom = y >= self.h - MARGIN;
        if !(at_left || at_right || at_top || at_bottom) {
            return None;
        }
        let me = self.self_rect();
        let vx = me.x as f64 + clamp(x, 0.0, self.w);
        let vy = me.y as f64 + clamp(y, 0.0, self.h);
        let (ix, iy) = (vx as i64, vy as i64);

        let mut best: Option<(String, f64, f64, i64)> = None;
        for name in self.peers.keys() {
            let Some(r) = self.layout.get(name) else {
                continue;
            };
            if name == &self.self_name {
                continue;
            }
            // 光标在哪条边上，就只找与这条边相接的邻居。
            let cand = if at_left && (me.left() - r.right()).abs() <= TOL {
                Rect::span_overlap((me.top(), me.bottom()), (r.top(), r.bottom()))
                    .map(|seg| (Edge::Left, seg))
            } else if at_right && (me.right() - r.left()).abs() <= TOL {
                Rect::span_overlap((me.top(), me.bottom()), (r.top(), r.bottom()))
                    .map(|seg| (Edge::Right, seg))
            } else if at_top && (me.top() - r.bottom()).abs() <= TOL {
                Rect::span_overlap((me.left(), me.right()), (r.left(), r.right()))
                    .map(|seg| (Edge::Top, seg))
            } else if at_bottom && (me.bottom() - r.top()).abs() <= TOL {
                Rect::span_overlap((me.left(), me.right()), (r.left(), r.right()))
                    .map(|seg| (Edge::Bottom, seg))
            } else {
                None
            };
            let Some((edge, seg)) = cand else {
                continue;
            };
            if !Rect::on_segment(edge, seg, ix, iy) {
                continue;
            }
            let (px, py) = match edge {
                Edge::Left => (r.right() as f64, vy),
                Edge::Right => (r.left() as f64, vy),
                Edge::Top => (vx, r.bottom() as f64),
                Edge::Bottom => (vx, r.top() as f64),
            };
            let seg_len = seg.1 - seg.0;
            let better = match &best {
                None => true,
                // 共享段更长的优先；等长取名字序，保证行为确定。
                Some((n, _, _, len)) => seg_len > *len || (seg_len == *len && name < n),
            };
            if better {
                best = Some((name.clone(), px, py, seg_len));
            }
        }
        best.map(|(n, x, y, _)| (n, x, y))
    }

    fn hand_off(&mut self, peer: &str, vx: f64, vy: f64, x: f64, y: f64) {
        if !self.peers.contains_key(peer) {
            return;
        }
        if !self.send(peer, Frame::Enter { x: vx, y: vy }) {
            return;
        }
        self.mode = Mode::Forwarding {
            peer: peer.to_string(),
        };
        self.last_abs = None;
        self.hide();
        // 光标拉回贴边内侧的中性位置，别让它真的停在边线上。
        let nx = if x <= MARGIN {
            NEUTRAL
        } else if x >= self.w - MARGIN {
            self.w - NEUTRAL
        } else {
            x
        };
        let ny = if y <= MARGIN {
            NEUTRAL
        } else if y >= self.h - MARGIN {
            self.h - NEUTRAL
        } else {
            y
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
    fn controlled(&mut self, peer: &str, exit: Edge, ev: &EventType) {
        match ev {
            EventType::MouseMove { x, y } => {
                if let (Some((ix, iy)), Some(t)) = (self.injected_pos, self.injected_at) {
                    if (x - ix).abs() < 2.0 && (y - iy).abs() < 2.0 && t.elapsed() < Duration::from_millis(250)
                    {
                        return;
                    }
                }
                self.reclaim_from_controlled(peer, exit, (*x, *y));
            }
            EventType::KeyPress(k) => {
                if let Some((ik, t)) = self.injected_key {
                    if ik == *k && t.elapsed() < Duration::from_millis(120) {
                        return;
                    }
                }
                self.reclaim_from_controlled(peer, exit, self.cur);
            }
            EventType::ButtonPress(b) => {
                if let Some((ib, t)) = self.injected_button {
                    if ib == *b && t.elapsed() < Duration::from_millis(120) {
                        return;
                    }
                }
                self.reclaim_from_controlled(peer, exit, self.cur);
            }
            _ => {}
        }
    }

    /// 被控方把控制还给控制方。pos 是触发归还的本机光标位置（可能已在边线上）。
    fn reclaim_from_controlled(&mut self, peer: &str, exit: Edge, pos: (f64, f64)) {
        let me = self.self_rect();
        // 本机屏幕坐标 -> 虚拟画布坐标（稍微越过边线一点，控制方映射回去正好贴边）。
        let push = match exit {
            Edge::Left => -2.0,
            Edge::Right => 2.0,
            Edge::Top => -2.0,
            Edge::Bottom => 2.0,
        };
        let (sx, sy) = match exit {
            Edge::Left | Edge::Right => {
                let nx = if exit == Edge::Left { 0.0 } else { self.w };
                (nx + push, clamp(pos.1, 0.0, self.h))
            }
            _ => {
                let ny = if exit == Edge::Top { 0.0 } else { self.h };
                (clamp(pos.0, 0.0, self.w), ny + push)
            }
        };
        let vx = me.x as f64 + sx * (me.w as f64 / self.w);
        let vy = me.y as f64 + sy * (me.h as f64 / self.h);
        self.send(peer, Frame::Leave { x: vx, y: vy });
        self.mode = Mode::Local;
        self.unhide();
        self.last_abs = None;
        self.edge_streak = 0;
    }

    /// 主动收回（Ctrl+Alt+Esc）：把控制从被控机器上拿回来。
    fn reclaim(&mut self, peer: &str) {
        self.send(
            peer,
            Frame::Leave {
                x: self.cur.0,
                y: self.cur.1,
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
            Frame::Enter { x, y } => {
                let exit = self.border_toward(peer);
                let (lx, ly) = self.virtual_to_local(x, y);
                self.cur = (lx, ly);
                self.mode = Mode::Controlled {
                    peer: peer.to_string(),
                    exit,
                };
                self.last_abs = None;
                self.inject(&EventType::MouseMove { x: lx, y: ly });
            }
            Frame::Leave { x, y } => {
                let mine = matches!(&self.mode, Mode::Forwarding { peer: p } if p == peer);
                if !mine {
                    return;
                }
                self.mode = Mode::Local;
                self.unhide();
                let (lx, ly) = self.virtual_to_local(x, y);
                self.warp(lx, ly);
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

    /// 虚拟画布坐标 -> 本机屏幕坐标（按矩形与屏幕的比例缩放）。
    fn virtual_to_local(&self, vx: f64, vy: f64) -> (f64, f64) {
        let me = self.self_rect();
        let lx = (vx - me.x as f64) / me.w as f64 * self.w;
        let ly = (vy - me.y as f64) / me.h as f64 * self.h;
        (clamp(lx, 1.0, self.w - 1.0), clamp(ly, 1.0, self.h - 1.0))
    }

    /// 本机矩形通向某个邻居的那条边（对方跟我哪条边相接）。
    fn border_toward(&self, peer: &str) -> Edge {
        let me = self.self_rect();
        if let Some(r) = self.layout.get(peer) {
            if let Some((edge, _)) = Rect::shared_border(&me, r, TOL) {
                return edge;
            }
        }
        // 布局没对上时的兜底：按对方中心相对我中心的方向猜。
        match self.layout.get(peer) {
            Some(r) => {
                let dx = (r.x + r.w as i64 / 2) - (me.x + me.w as i64 / 2);
                let dy = (r.y + r.h as i64 / 2) - (me.y + me.h as i64 / 2);
                if dx.abs() >= dy.abs() {
                    if dx >= 0 { Edge::Right } else { Edge::Left }
                } else if dy >= 0 {
                    Edge::Bottom
                } else {
                    Edge::Top
                }
            }
            None => Edge::Right,
        }
    }

    fn apply_input(&mut self, peer: &str, ev: InputEvent) {
        let (dx, dy) = delta_of(&ev);
        if dx != 0.0 || dy != 0.0 {
            let nx = self.cur.0 + dx;
            let ny = self.cur.1 + dy;
            let exit = match self.mode {
                Mode::Controlled { exit, .. } => exit,
                _ => return,
            };
            let outside = match exit {
                Edge::Left => nx < 0.0,
                Edge::Right => nx > self.w - 1.0,
                Edge::Top => ny < 0.0,
                Edge::Bottom => ny > self.h - 1.0,
            };
            if outside {
                // 继续往回推：把控制还给对方。
                let pos = (clamp(nx, 0.0, self.w), clamp(ny, 0.0, self.h));
                self.reclaim_from_controlled(peer, exit, pos);
                return;
            }
            self.cur = (clamp(nx, 0.0, self.w - 1.0), clamp(ny, 0.0, self.h - 1.0));
        }
        if let Some(local) = to_local(&ev, self.cur) {
            self.inject(&local);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn session(layout: Vec<(&str, Rect)>, self_name: &str) -> Session {
        let mut map = HashMap::new();
        for (n, r) in layout {
            map.insert(n.to_string(), r);
        }
        let mut s = Session::new(1920.0, 1080.0, self_name.to_string(), map);
        // 布局里的其他机器都当作已连接（exit_target 只考虑在线邻居）。
        let names: Vec<String> = s.layout.keys().filter(|n| n.as_str() != self_name).cloned().collect();
        for n in names {
            let (tx, _rx) = std::sync::mpsc::channel();
            s.peers.insert(n, tx);
        }
        s
    }

    #[test]
    fn right_neighbor_full_edge() {
        let s = session(
            vec![
                ("me", Rect::new(0, 0, 1920, 1080)),
                ("b", Rect::new(1920, 0, 1920, 1080)),
            ],
            "me",
        );
        let t = s.exit_target(1919.5, 540.0);
        assert_eq!(t.as_ref().map(|(n, _, _)| n.as_str()), Some("b"));
        let (_, vx, vy) = t.unwrap();
        assert_eq!((vx, vy), (1920.0, 540.0));
    }

    #[test]
    fn partial_edge_offset_screens() {
        // 截图场景：右侧屏幕比本机矮、只和本机上半部分相接。
        let s = session(
            vec![
                ("me", Rect::new(0, 0, 1920, 1080)),
                ("r", Rect::new(1920, -200, 1080, 700)),
            ],
            "me",
        );
        // 光标在上半部分：能跨
        assert!(s.exit_target(1919.5, 300.0).is_some());
        // 光标在下半部分：够不到（y=900 超出对方的段 [-200, 500]）
        assert!(s.exit_target(1919.5, 900.0).is_none());
    }

    #[test]
    fn same_side_two_neighbors() {
        // 左侧错位并排两台：l1 与本机上半段相接，l2 与下半段相接
        let s = session(
            vec![
                ("me", Rect::new(0, 0, 1920, 1080)),
                ("l1", Rect::new(-1920, -540, 1920, 1080)),
                ("l2", Rect::new(-1920, 540, 1920, 1080)),
            ],
            "me",
        );
        assert_eq!(
            s.exit_target(0.5, 200.0).map(|(n, _, _)| n),
            Some("l1".to_string())
        );
        assert_eq!(
            s.exit_target(0.5, 900.0).map(|(n, _, _)| n),
            Some("l2".to_string())
        );
    }

    #[test]
    fn gap_within_tolerance() {
        let s = session(
            vec![
                ("me", Rect::new(0, 0, 1920, 1080)),
                ("b", Rect::new(1922, 0, 1920, 1080)),
            ],
            "me",
        );
        assert!(s.exit_target(1919.5, 500.0).is_some());
    }

    #[test]
    fn vertical_neighbor_maps_entry() {
        let s = session(
            vec![
                ("me", Rect::new(0, 0, 1920, 1080)),
                ("t", Rect::new(0, -1080, 1920, 1080)),
            ],
            "me",
        );
        let t = s.exit_target(960.0, 0.5);
        assert_eq!(t.as_ref().map(|(n, _, _)| n.as_str()), Some("t"));
        let (_, vx, vy) = t.unwrap();
        assert_eq!((vx, vy), (960.0, 0.0));
        // 进入点映回本机：虚拟 960,0 → 屏幕 960,1
        assert_eq!(s.virtual_to_local(960.0, 0.0), (960.0, 1.0));
    }

    #[test]
    fn virtual_to_local_scales() {
        // 邻居分辨率不同（画布 1280x720，本机 1920x1080）
        let s = session(
            vec![
                ("me", Rect::new(0, 0, 1280, 720)),
                ("b", Rect::new(1280, 0, 1920, 1080)),
            ],
            "me",
        );
        // 注意：session 的 w/h 是"本机屏幕"=1280x720
        let s = Session::new(1280.0, 720.0, "me".into(), s.layout.clone());
        assert_eq!(s.virtual_to_local(640.0, 360.0), (640.0, 360.0));
        let (x, y) = s.virtual_to_local(0.0, 720.0);
        assert_eq!((x, y), (1.0, 719.0));
    }
}
