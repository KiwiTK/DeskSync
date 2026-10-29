use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 一台机器在虚拟画布上的矩形（虚拟像素）。
/// 本机放在 (0,0)，邻居可以摆在任意位置，只要与谁有公共边就能跨到谁。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: i64, y: i64, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn left(&self) -> i64 {
        self.x
    }
    pub fn right(&self) -> i64 {
        self.x + self.w as i64
    }
    pub fn top(&self) -> i64 {
        self.y
    }
    pub fn bottom(&self) -> i64 {
        self.y + self.h as i64
    }

    /// 两条边（沿一个轴的区间）的重叠范围；不重叠返回 None。
    /// 容差 gap 允许摆放时留几像素缝隙或轻微交叠。
    pub fn span_overlap(a: (i64, i64), b: (i64, i64)) -> Option<(i64, i64)> {
        let lo = a.0.max(b.0);
        let hi = a.1.min(b.1);
        if hi > lo {
            Some((lo, hi))
        } else {
            None
        }
    }

    /// a 的哪条边与 b 相接？返回 (a 的边, 共享段在另一轴上的范围)。
    /// gap 是边对齐容差（像素）。
    pub fn shared_border(a: &Rect, b: &Rect, gap: i64) -> Option<(Edge, (i64, i64))> {
        let v_overlap = Self::span_overlap((a.top(), a.bottom()), (b.top(), b.bottom()));
        let h_overlap = Self::span_overlap((a.left(), a.right()), (b.left(), b.right()));
        // 左右相接：竖直方向有重叠，水平方向边距在容差内
        if let Some(seg) = v_overlap {
            if (a.right() - b.left()).abs() <= gap {
                return Some((Edge::Right, seg));
            }
            if (a.left() - b.right()).abs() <= gap {
                return Some((Edge::Left, seg));
            }
        }
        if let Some(seg) = h_overlap {
            if (a.bottom() - b.top()).abs() <= gap {
                return Some((Edge::Bottom, seg));
            }
            if (a.top() - b.bottom()).abs() <= gap {
                return Some((Edge::Top, seg));
            }
        }
        None
    }

    /// 点（虚拟坐标）是否落在这条边的共享段内。
    pub fn on_segment(edge: Edge, seg: (i64, i64), x: i64, y: i64) -> bool {
        let v = match edge {
            Edge::Left | Edge::Right => y,
            Edge::Top | Edge::Bottom => x,
        };
        v >= seg.0 && v <= seg.1
    }
}

/// 本机屏幕的哪一条边通向邻居。旧版配置用整条边对应一个邻居，
/// 新版布局改用 Rect 自由摆放，这个枚举只剩迁移和内部标记用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    #[allow(dead_code)]
    pub fn opposite(self) -> Edge {
        match self {
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
        }
    }

    #[allow(dead_code)]
    pub fn parse(s: &str) -> Option<Edge> {
        match s.to_ascii_lowercase().as_str() {
            "left" | "l" => Some(Edge::Left),
            "right" | "r" => Some(Edge::Right),
            "top" | "t" | "up" => Some(Edge::Top),
            "bottom" | "b" | "down" => Some(Edge::Bottom),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    /// 形如 "192.168.1.20:24987"，留空则依赖局域网发现。
    #[serde(default)]
    pub addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 本机名称，广播与配对时显示。
    pub name: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// 旧版「邻居 -> 本机通向它的边」。新版本布局改用 layout；
    /// 字段保留是为了老配置能读进来做迁移。
    #[serde(default)]
    pub edges: HashMap<String, Edge>,
    /// 每台机器在虚拟画布上的摆放（含本机自己，键为本机名）。
    #[serde(default)]
    pub layout: HashMap<String, Rect>,
    #[serde(default)]
    pub peers: Vec<Peer>,
    /// 已确认的邻居证书指纹（十六进制 SHA-256）。
    #[serde(default)]
    pub trusted: HashMap<String, String>,
    /// 首次连接时自动信任对方。家用局域网可以省掉确认步骤。
    #[serde(default)]
    pub auto_trust: bool,
    /// 手动指定虚拟桌面尺寸，多显示器时可覆盖自动探测结果。
    #[serde(default)]
    pub screen: Option<(u32, u32)>,
}

fn default_port() -> u16 {
    24987
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: default_name(),
            port: default_port(),
            edges: HashMap::new(),
            layout: HashMap::new(),
            peers: Vec::new(),
            trusted: HashMap::new(),
            auto_trust: true,
            screen: None,
        }
    }
}

fn default_name() -> String {
    let host = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    host.unwrap_or_else(|| "desksync".to_string())
}

pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        dirs_home().join("Library/Application Support/DeskSync")
    } else if cfg!(target_os = "windows") {
        let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into());
        PathBuf::from(base).join("DeskSync")
    } else {
        let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
            dirs_home().join(".config").to_string_lossy().to_string()
        });
        PathBuf::from(base).join("desksync")
    }
}

fn dirs_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

impl Config {
    pub fn path() -> PathBuf {
        data_dir().join("config.json")
    }

    /// 保证 layout 里包含本机和所有邻居：
    /// - 本机固定在 (0,0)，尺寸就是本机屏幕；
    /// - 邻居缺位置时按旧版 edges 翻译成默认摆放，没有旧配置就放右边。
    /// 迁移的结果写不写回由调用方决定（UI 保存时自然会写）。
    pub fn ensure_layout(&mut self, screen: (u32, u32)) {
        let (w, h) = screen;
        self.layout
            .entry(self.name.clone())
            .or_insert_with(|| Rect::new(0, 0, w, h));
        let me = self.layout.get(&self.name).copied().unwrap_or(Rect::new(0, 0, w, h));
        let edges = self.edges.clone();
        for peer in &self.peers {
            if self.layout.contains_key(&peer.name) {
                continue;
            }
            let rect = match edges.get(&peer.name) {
                Some(Edge::Left) => Rect::new(me.left() - w as i64, me.y, w, h),
                Some(Edge::Right) => Rect::new(me.right(), me.y, w, h),
                Some(Edge::Top) => Rect::new(me.x, me.top() - h as i64, w, h),
                Some(Edge::Bottom) => Rect::new(me.x, me.bottom(), w, h),
                None => Rect::new(me.right(), me.y, w, h),
            };
            self.layout.insert(peer.name.clone(), rect);
        }
        // 删掉的邻居的矩形也清掉，免得配置越积越脏。
        self.layout.retain(|k, _| k == &self.name || self.peers.iter().any(|p| &p.name == k));
    }

    pub fn load() -> std::io::Result<Self> {
        let text = std::fs::read_to_string(Self::path())?;
        let mut cfg: Config = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // 0.2.3 起 DeskSync 默认端口从 24872 改为 24987（24872 与 MouseShare 冲突）。
        // 老配置里写死的旧默认端口自动迁移。
        if cfg.port == 24872 {
            cfg.port = default_port();
        }
        Ok(cfg)
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = data_dir();
        std::fs::create_dir_all(&dir)?;
        let text = serde_json::to_string_pretty(self).unwrap();
        std::fs::write(Self::path(), text)
    }
}
