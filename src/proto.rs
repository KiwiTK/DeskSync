use serde::{Deserialize, Serialize};

/// 网络上传输的输入事件。
///
/// 鼠标用相对位移而不是绝对坐标：两台机器分辨率不同，绝对坐标没法直接用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    Move { dx: f64, dy: f64 },
    KeyPress(rdev::Key),
    KeyRelease(rdev::Key),
    ButtonPress(rdev::Button),
    ButtonRelease(rdev::Button),
    Wheel { delta_x: i64, delta_y: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Frame {
    /// 握手后立刻发出的自我介绍。screen 是本机屏幕尺寸，用来把布局里的
    /// 矩形尺寸自动校正成对方的真实分辨率。
    Hello {
        name: String,
        #[serde(default)]
        screen: Option<(u32, u32)>,
    },
    /// 我把控制交给你：`x`/`y` 是虚拟画布上的交出点（双方布局一致时
    /// 这个点就落在两边共享的边上）。
    Enter { x: f64, y: f64 },
    /// 归还控制，附带归还点的虚拟画布坐标。
    Leave { x: f64, y: f64 },
    /// 转发的键鼠事件。
    Input(InputEvent),
    /// 剪贴板文本同步。
    Clipboard { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Beacon {
    pub name: String,
    pub port: u16,
}

/// 把 rdev 捕获到的事件翻译成网络事件。绝对坐标在这里转成相对位移。
pub fn to_network(ev: &rdev::EventType, last: Option<(f64, f64)>) -> Option<(InputEvent, (f64, f64))> {
    use rdev::EventType::*;
    match ev {
        MouseMove { x, y } => {
            let (dx, dy) = match last {
                Some((lx, ly)) => (x - lx, y - ly),
                None => (0.0, 0.0),
            };
            Some((InputEvent::Move { dx, dy }, (*x, *y)))
        }
        KeyPress(k) => Some((InputEvent::KeyPress(*k), last.unwrap_or((0.0, 0.0)))),
        KeyRelease(k) => Some((InputEvent::KeyRelease(*k), last.unwrap_or((0.0, 0.0)))),
        ButtonPress(b) => Some((InputEvent::ButtonPress(*b), last.unwrap_or((0.0, 0.0)))),
        ButtonRelease(b) => Some((InputEvent::ButtonRelease(*b), last.unwrap_or((0.0, 0.0)))),
        Wheel { delta_x, delta_y } => Some((
            InputEvent::Wheel {
                delta_x: *delta_x,
                delta_y: *delta_y,
            },
            last.unwrap_or((0.0, 0.0)),
        )),
    }
}

/// 把网络事件还原成本地注入用的 rdev 事件。
pub fn to_local(ev: &InputEvent, pos: (f64, f64)) -> Option<rdev::EventType> {
    use rdev::EventType::*;
    match ev {
        InputEvent::Move { .. } => Some(MouseMove { x: pos.0, y: pos.1 }),
        InputEvent::KeyPress(k) => Some(KeyPress(*k)),
        InputEvent::KeyRelease(k) => Some(KeyRelease(*k)),
        InputEvent::ButtonPress(b) => Some(ButtonPress(*b)),
        InputEvent::ButtonRelease(b) => Some(ButtonRelease(*b)),
        InputEvent::Wheel { delta_x, delta_y } => Some(Wheel {
            delta_x: *delta_x,
            delta_y: *delta_y,
        }),
    }
}

pub fn delta_of(ev: &InputEvent) -> (f64, f64) {
    match ev {
        InputEvent::Move { dx, dy } => (*dx, *dy),
        _ => (0.0, 0.0),
    }
}
