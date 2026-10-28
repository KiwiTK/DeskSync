//! 交出控制时把本机光标藏起来，收回时再显示。
//! 各平台都是几行系统调用，不引入额外依赖。

pub fn hide() {
    unsafe { hide_impl() }
}

pub fn show() {
    unsafe { show_impl() }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn CGDisplayHideCursor(display: u32);
    fn CGDisplayShowCursor(display: u32);
}

#[cfg(target_os = "macos")]
unsafe fn hide_impl() {
    CGDisplayHideCursor(0);
}

#[cfg(target_os = "macos")]
unsafe fn show_impl() {
    CGDisplayShowCursor(0);
}

#[cfg(target_os = "windows")]
extern "system" {
    fn ShowCursor(show: i32) -> i32;
}

#[cfg(target_os = "windows")]
unsafe fn hide_impl() {
    let mut n = ShowCursor(0);
    while n >= 0 {
        n = ShowCursor(0);
    }
}

#[cfg(target_os = "windows")]
unsafe fn show_impl() {
    let mut n = ShowCursor(1);
    while n < 0 {
        n = ShowCursor(1);
    }
}

#[cfg(target_os = "linux")]
unsafe fn hide_impl() {}

#[cfg(target_os = "linux")]
unsafe fn show_impl() {}
