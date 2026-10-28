use std::sync::mpsc::Sender;

use crate::proto::Frame;

pub struct Clipboard {
    last: String,
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            last: String::new(),
        }
    }

    /// 轮询本机剪贴板，内容变了就广播给所有已连接的邻居。
    pub fn poll(&mut self, senders: &[Sender<Frame>]) {
        let text = read_text();
        let current = text.unwrap_or_default();
        if current != self.last {
            self.last = current.clone();
            if !current.is_empty() {
                for tx in senders {
                    let _ = tx.send(Frame::Clipboard {
                        text: current.clone(),
                    });
                }
            }
        }
    }

    /// 收到远程剪贴板后写入本机，并记为本机已知内容，避免来回同步。
    pub fn adopt(&mut self, text: &str) {
        self.last = text.to_string();
        write_text(text);
    }
}

fn read_text() -> Option<String> {
    let mut board = arboard::Clipboard::new().ok()?;
    board.get_text().ok()
}

fn write_text(text: &str) {
    if let Ok(mut board) = arboard::Clipboard::new() {
        let _ = board.set_text(text.to_string());
    }
}
