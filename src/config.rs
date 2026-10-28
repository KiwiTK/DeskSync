use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// 本机屏幕的哪一条边通向邻居。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
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
    /// 形如 "192.168.1.20:24872"，留空则依赖局域网发现。
    #[serde(default)]
    pub addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 本机名称，广播与配对时显示。
    pub name: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// 邻居名 -> 本机通向它的那条边。
    #[serde(default)]
    pub edges: HashMap<String, Edge>,
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
    24872
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: default_name(),
            port: default_port(),
            edges: HashMap::new(),
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

    pub fn load() -> std::io::Result<Self> {
        let text = std::fs::read_to_string(Self::path())?;
        serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = data_dir();
        std::fs::create_dir_all(&dir)?;
        let text = serde_json::to_string_pretty(self).unwrap();
        std::fs::write(Self::path(), text)
    }
}
