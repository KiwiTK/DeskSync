//! 屏幕几何，Windows 多显示器下的坐标纠偏。
//!
//! rdev 在 Windows 上有个坑：`display_size()` 读的是 `SM_CXSCREEN/SM_CYSCREEN`，
//! **只返回主显示器的尺寸**；而鼠标钩子报的坐标和 `simulate` 注入的坐标
//! 都是「虚拟桌面」坐标（副屏摆在主屏左边/上边时甚至为负）。
//!
//! 于是多显示器机器上会出现：屏幕宽度只按 1920 算，但光标能跑到 3000+，
//! `x >= w - 1` 这个贴边判定在两台显示器之间的缝隙处就成立 —— 跨屏要么在
//! 显示器接缝处乱触发，要么永远触发不了。
//!
//! 这里直接读虚拟桌面矩形（SM_*VIRTUALSCREEN），把尺寸和原点一起交给调用方：
//! 捕获到的坐标减掉原点，注入时加回原点。

#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    /// 虚拟桌面左上角在鼠标坐标系里的位置（主屏左上为原点，所以可能为负）。
    pub x: i64,
    pub y: i64,
    pub w: u64,
    pub h: u64,
}

#[cfg(windows)]
pub fn geometry() -> Option<Geometry> {
    const SM_XVIRTUALSCREEN: i32 = 76;
    const SM_YVIRTUALSCREEN: i32 = 77;
    const SM_CXVIRTUALSCREEN: i32 = 78;
    const SM_CYVIRTUALSCREEN: i32 = 79;

    #[link(name = "user32")]
    extern "system" {
        fn GetSystemMetrics(n_index: i32) -> i32;
    }

    unsafe {
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN) as i64;
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN) as i64;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some(Geometry {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN) as i64,
            y: GetSystemMetrics(SM_YVIRTUALSCREEN) as i64,
            w: w as u64,
            h: h as u64,
        })
    }
}

/// 其他平台：rdev 的 display_size() 已经是一整块屏幕，没有虚拟桌面原点的问题。
#[cfg(not(windows))]
pub fn geometry() -> Option<Geometry> {
    None
}
