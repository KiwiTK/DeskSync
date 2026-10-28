//! 轻量诊断日志：写到配置目录下的 `desksync.log`。
//!
//! Windows 上界面是 GUI 子系统，stderr 是黑洞；后台服务又被 spawn 成
//! stdout/stderr 全 null。出了问题唯一能留下线索的就是这个文件。
//! 界面启动失败时还会把错误弹进原生消息框，让人不用翻文件也能看到。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config;

/// 超过这个大小就从头重写，避免无限膨胀。
const MAX_BYTES: u64 = 512 * 1024;

static LOG_LOCK: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    config::data_dir().join("desksync.log")
}

pub fn write(line: &str) {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file = path();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // 注意：OpenOptions 默认是只读，只给 create 不给 write 会打开成功但写不进去。
    let append = std::fs::metadata(&file).map(|m| m.len() < MAX_BYTES).unwrap_or(true);
    let mut opts = OpenOptions::new();
    opts.create(true);
    if append {
        opts.append(true);
    } else {
        opts.write(true).truncate(true);
    }
    if let Ok(mut f) = opts.open(&file) {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{secs}] {line}");
    }
}

/// 装上 panic 钩子并记录本次启动。
/// 必须在 main 里最先调用：GUI 进程崩溃时这是唯一的现场。
pub fn init() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let text = format!("PANIC: {info}\n堆栈:\n{bt}");
        write(&text);
        // Windows 的 GUI 进程看不到 stderr，必须把崩溃摆到人面前。
        show_error(&format!("DeskSync 遇到内部错误，进程即将结束：\n\n{info}"));
        default_hook(info);
    }));
    write(&format!(
        "==== 启动 v{} pid={} exe={:?} args={:?} ui={}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        std::env::args().collect::<Vec<_>>(),
        cfg!(feature = "ui"),
    ));
}

pub fn path_display() -> String {
    path().display().to_string()
}

/// Windows 的 GUI 进程没有控制台可写，出大事时弹一个原生消息框。
#[cfg(windows)]
pub fn show_error(text: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let text = wide(&format!("{text}\n\n详细日志：{}", path_display()));
    let caption = wide("DeskSync");
    unsafe {
        MessageBoxW(0, text.as_ptr(), caption.as_ptr(), 0x10); // MB_ICONERROR
    }
}

#[cfg(not(windows))]
pub fn show_error(_text: &str) {}
