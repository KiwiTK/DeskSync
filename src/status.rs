//! UI 进程和后台守护之间的最小约定：两个 JSON 文件。
//!
//! - `status.json`：守护进程定期写，UI 只读，用来判断它还活着没有。
//! - `command.json`：UI 写，守护进程读到就立刻删掉。
//!
//! 用文件而不是 socket，是为了让两边互不依赖启动顺序：谁先起来都行，
//! 守护进程崩溃了 UI 也不会崩，UI 退了守护进程照样干活。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn status_path() -> PathBuf {
    crate::config::data_dir().join("status.json")
}

pub fn command_path() -> PathBuf {
    crate::config::data_dir().join("command.json")
}

/// 状态陈旧到这个程度就认为守护进程已经不在了。
pub const STALE_MS: u64 = 3000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 局域网上广播发现到的邻居。UI 拿它做一键添加，省得手打 IP。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovered {
    pub name: String,
    pub addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub name: String,
    pub port: u16,
    pub fingerprint: String,
    pub screen: (u32, u32),
    pub paused: bool,
    /// 后台服务自己的版本号。和界面版本对不上，说明装了新版但旧后台还赖着，跨屏不会生效。
    #[serde(default)]
    pub version: String,
    /// 后台服务的 pid：界面照着它就能把不肯退的旧进程干掉。
    #[serde(default)]
    pub pid: u32,
    /// 广播发现到的邻居（名字 + ip:port）。
    #[serde(default)]
    pub discovered: Vec<Discovered>,
    /// `local`：操作本机；`forwarding:name`：正在控制谁；`controlled:name`：被谁控制。
    pub mode: String,
    pub connected: Vec<String>,
    pub written_ms: u64,
}

/// UI 发给守护进程的指令。字段都是可选的，写了什么就执行什么。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiCommand {
    /// 暂停 / 继续共享（暂停后本机输入不再转发，也不接受接管）
    #[serde(default)]
    pub pause: Option<bool>,
    /// 立刻把控制权收回来
    #[serde(default)]
    pub reclaim: bool,
    /// 让守护进程退出
    #[serde(default)]
    pub quit: bool,
}

impl Status {
    pub fn is_forwarding(&self) -> Option<&str> {
        self.mode.strip_prefix("forwarding:")
    }

    pub fn is_controlled(&self) -> Option<&str> {
        self.mode.strip_prefix("controlled:")
    }
}

pub fn write_status(status: &Status) -> std::io::Result<()> {
    let mut s = status.clone();
    s.written_ms = now_ms();
    let dir = crate::config::data_dir();
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(&s).unwrap();
    std::fs::write(status_path(), text)
}

pub fn read_status() -> Option<Status> {
    let text = std::fs::read_to_string(status_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// 守护进程是否还活着：状态文件在，而且足够新。
pub fn daemon_alive() -> bool {
    match read_status() {
        Some(s) => now_ms().saturating_sub(s.written_ms) < STALE_MS,
        None => false,
    }
}

pub fn send_command(cmd: &UiCommand) -> std::io::Result<()> {
    let dir = crate::config::data_dir();
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string(cmd).unwrap();
    std::fs::write(command_path(), text)
}

/// 守护进程调用：取出待处理的指令，取到就删掉文件。
pub fn take_command() -> Option<UiCommand> {
    let text = std::fs::read_to_string(command_path()).ok()?;
    let _ = std::fs::remove_file(command_path());
    serde_json::from_str(&text).ok()
}
