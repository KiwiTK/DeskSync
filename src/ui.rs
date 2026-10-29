//! 图形界面：菜单栏图标 + 设置窗口。
//!
//! 界面是单独一层，不直接参与键鼠捕获和网络收发——它只读 `status.json`、
//! 写 `config.json` 和 `command.json`，后台服务负责其余部分。
//! 这样界面崩了、被关掉了，共享本身都不受影响。

use crate::config::{Config, Edge, Peer, Rect};
use crate::status;
use eframe::egui;
use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

const ICON_PNG: &[u8] = include_bytes!("../assets/icon-128.png");

pub fn run() -> io::Result<()> {
    crate::log::write("界面进程启动");
    let cfg = Config::load().unwrap_or_default();

    // 后台服务没在跑就先拉起来，界面退出后它继续干活。
    if !status::daemon_alive() {
        crate::log::write("后台未运行，正在拉起");
        spawn_daemon()?;
        std::thread::sleep(Duration::from_millis(700));
    } else {
        crate::log::write("后台已在运行");
    }

    // 托盘不能在这里建：macOS 的 NSStatusItem 要等 NSApplication 起来之后才能创建，
    // 在 run_native 之前碰它会直接崩在 CGSConnectionByID。所以推迟到第一帧。
    crate::log::write("进入图形事件循环");
    let opened = eframe::run_native(
        "DeskSync",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([640.0, 720.0])
                .with_min_inner_size([520.0, 560.0])
                .with_icon(load_icon()),
            ..Default::default()
        },
        Box::new(move |cc| {
            install_cjk_fonts(&cc.egui_ctx);
            Ok(Box::new(UiApp::new(cfg)))
        }),
    );
    if let Err(e) = &opened {
        // 界面起不来也要留下线索：写日志，Windows 上再弹个框让人看得见。
        let log_path = crate::log::path_display();
        crate::log::write(&format!("图形事件循环启动失败: {e}"));
        crate::log::show_error(&format!(
            "DeskSync 界面启动失败：{e}\n\n后台共享服务不受影响，仍在运行。\n\n详细日志：{log_path}"
        ));
    } else {
        crate::log::write("图形事件循环正常退出");
    }
    // 窗口真正关掉后进程必须跟着走，不能赖在菜单栏里。
    std::process::exit(0);
}

/// egui 内置字体不含 CJK 字形，中文会渲染成方块。
/// 把系统里现成的中文字体追加为回退字体（排在默认字体后面，只接管汉字）。
fn install_cjk_fonts(ctx: &egui::Context) {
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/STHeiti Medium.ttc",
            "/System/Library/Fonts/Supplemental/Songti.ttc",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            "C:\\Windows\\Fonts\\msyh.ttc",
            "C:\\Windows\\Fonts\\msyh.ttf",
            "C:\\Windows\\Fonts\\simsun.ttc",
            "C:\\Windows\\Fonts\\simhei.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/wenquanyi-microhei/wqy-microhei.ttc",
        ]
    };

    for path in candidates {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("system-cjk".to_owned(), egui::FontData::from_owned(data).into());
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let list = fonts.families.entry(family).or_default();
            // 放到最前面：行高按它的度量算，中英文混排才不会叠行。
            // CJK 字体自带拉丁字形，英文显示不受影响。
            list.insert(0, "system-cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        return;
    }
    eprintln!("未找到系统中文字体，界面上的中文可能显示为方块");
}

fn load_icon() -> egui::viewport::IconData {
    let img = image::load_from_memory(ICON_PNG)
        .map(|i| i.to_rgba8())
        .expect("内置图标损坏");
    let (w, h) = img.dimensions();
    egui::viewport::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    }
}

/// 托盘项 Identity。egui 版本不再自带菜单，用 tray-icon 自己的。
struct Tray {
    _icon: tray_icon::TrayIcon,
    open: tray_icon::menu::MenuItem,
    pause: tray_icon::menu::MenuItem,
    reclaim: tray_icon::menu::MenuItem,
    quit: tray_icon::menu::MenuItem,
}

fn build_tray() -> io::Result<Tray> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tray_icon::TrayIconBuilder;

    let open = MenuItem::with_id("open", "打开 DeskSync", true, None);
    let pause = MenuItem::with_id("pause", "暂停共享", true, None);
    let reclaim = MenuItem::with_id("reclaim", "收回控制", true, None);
    let quit = MenuItem::with_id("quit", "退出 DeskSync", true, None);

    let menu = Menu::with_items(&[
        &MenuItem::with_id("title", "DeskSync", false, None),
        &PredefinedMenuItem::separator(),
        &open,
        &pause,
        &reclaim,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    let img = image::load_from_memory(ICON_PNG)
        .map(|i| i.to_rgba8())
        .expect("内置图标损坏");
    let (w, h) = img.dimensions();
    let icon = tray_icon::Icon::from_rgba(img.into_raw(), w, h)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("DeskSync")
        .with_icon(icon)
        .build()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

    Ok(Tray {
        _icon: tray,
        open,
        pause,
        reclaim,
        quit,
    })
}

/// 界面上的一行草稿：正在填但还没保存的邻居。
#[derive(Default)]
struct Draft {
    name: String,
    addr: String,
}

struct UiApp {
    cfg: Config,
    status: Option<status::Status>,
    last_poll: Instant,
    notice: String,
    draft: Draft,
    /// 托盘延后到事件循环转起来之后再建；建失败就记下来，不再反复重试。
    tray: Option<Tray>,
    tray_error: Option<String>,
    /// 用户明确要求退出（托盘「退出」或 Cmd+Q），此时不再拦截关窗。
    quitting: bool,
    /// 布局画布的草稿（虚拟画布矩形，含本机）。保存时写进配置。
    layout_draft: HashMap<String, Rect>,
    /// 后台服务自报的版本与 pid：版本和本程序对不上，说明旧后台还赖着。
    daemon_version: String,
    daemon_pid: u32,
    /// 局域网广播发现到的邻居（名字 + ip:port）。
    discovered: Vec<status::Discovered>,
    /// 手动查出来的、占着本端口的旧进程 pid。
    port_blocked: Option<u32>,
}

impl UiApp {
    fn new(cfg: Config) -> Self {
        Self {
            cfg,
            status: None,
            last_poll: Instant::now() - Duration::from_secs(9),
            notice: String::new(),
            draft: Draft::default(),
            tray: None,
            tray_error: None,
            quitting: false,
            layout_draft: HashMap::new(),
            daemon_version: String::new(),
            daemon_pid: 0,
            discovered: Vec::new(),
            port_blocked: None,
        }
    }

    /// 彻底退出：先通知后台服务退出，等一小会儿还赖着就按 pid 强杀，然后自己关窗。
    fn quit_all(&mut self, ctx: &egui::Context) {
        self.poke(status::UiCommand {
            pause: None,
            reclaim: false,
            quit: true,
        });
        // 后台服务最多 300ms 后才读到指令，给它一秒时间自己走。
        std::thread::sleep(Duration::from_millis(900));
        if self.daemon_pid > 0 {
            let _ = kill_pid(self.daemon_pid);
        }
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// 后台服务版本和本程序不一致（装了新版但旧进程没死）时，按 pid 换掉它。
    fn restart_daemon(&mut self) {
        if self.daemon_pid > 0 {
            let _ = kill_pid(self.daemon_pid);
            std::thread::sleep(Duration::from_millis(400));
        }
        match spawn_daemon() {
            Ok(()) => self.notice = format!("后台服务已重启为 v{}。", env!("CARGO_PKG_VERSION")),
            Err(e) => self.notice = format!("重启失败: {e}"),
        }
        self.daemon_pid = 0;
        self.daemon_version.clear();
        self.status = None;
        self.last_poll = Instant::now() - Duration::from_secs(9);
    }

    /// 第一帧才建托盘：此时 NSApplication 已经在跑，NSStatusItem 可以安全创建。
    fn ensure_tray(&mut self) {
        if self.tray.is_some() || self.tray_error.is_some() {
            return;
        }
        match build_tray() {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => {
                eprintln!("创建菜单栏图标失败: {e}");
                self.tray_error = Some(e.to_string());
            }
        }
    }

    fn save(&mut self) -> io::Result<()> {
        self.cfg.layout = self.layout_draft.clone();
        self.cfg.ensure_layout(self.screen_size());
        self.layout_draft = self.cfg.layout.clone();
        self.cfg.save()?;
        self.notice = "已保存，布局立即生效；改名或换端口需要重启后台服务。".to_string();
        Ok(())
    }

    /// 本机屏幕尺寸：状态文件里有就用，否则按常见值兜底。
    fn screen_size(&self) -> (u32, u32) {
        self.status
            .as_ref()
            .map(|s| s.screen)
            .unwrap_or((1920, 1080))
    }

    fn poke(&self, cmd: status::UiCommand) {
        if let Err(e) = status::send_command(&cmd) {
            eprintln!("发送指令失败: {e}");
        }
    }

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        // macOS 上这些事件必须靠界面这一侧的事件循环来取，托盘自己没有线程。
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        // 先把事件收成动作：quit_all 要 &mut self，不能捏着 tray 的借用不放。
        enum Act {
            Open,
            Pause,
            Reclaim,
            Quit,
        }
        let mut acts: Vec<Act> = Vec::new();
        while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if ev.id == *tray.open.id() {
                acts.push(Act::Open);
            } else if ev.id == *tray.pause.id() {
                acts.push(Act::Pause);
            } else if ev.id == *tray.reclaim.id() {
                acts.push(Act::Reclaim);
            } else if ev.id == *tray.quit.id() {
                acts.push(Act::Quit);
            }
        }
        let _ = tray_icon::TrayIconEvent::receiver().try_recv();

        for act in acts {
            match act {
                Act::Open => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Act::Pause => {
                    let paused = self.status.as_ref().map(|s| s.paused).unwrap_or(false);
                    self.poke(status::UiCommand {
                        pause: Some(!paused),
                        reclaim: false,
                        quit: false,
                    });
                }
                Act::Reclaim => {
                    self.poke(status::UiCommand {
                        pause: None,
                        reclaim: true,
                        quit: false,
                    });
                }
                Act::Quit => self.quit_all(ctx),
            }
        }
    }
}

impl eframe::App for UiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_tray();
        self.handle_tray_events(ctx);

        if self.last_poll.elapsed() > Duration::from_millis(400) {
            self.last_poll = Instant::now();
            self.status = status::read_status();
            if let Some(s) = &self.status {
                self.daemon_version = s.version.clone();
                self.daemon_pid = s.pid;
                self.discovered = s.discovered.clone();
            }
        }

        // 点关闭只是藏到菜单栏；托盘「退出」或 Cmd+Q 才真的退出。
        let cmd_q = cfg!(target_os = "macos")
            && ctx.input(|i| {
                i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Key {
                            key: egui::Key::Q,
                            pressed: true,
                            modifiers,
                            ..
                        } if modifiers.command
                    )
                })
            });
        if cmd_q {
            self.quitting = true;
            self.poke(status::UiCommand {
                pause: None,
                reclaim: false,
                quit: true,
            });
        }
        if self.quitting {
            // 不拦 Close，让窗口真的关掉；run_native 返回后进程会 exit(0)。
        } else if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("DeskSync");
            ui.separator();

            self.draw_status(ui);

            ui.add_space(8.0);
            ui.heading("本机");
            ui.separator();
            self.draw_self(ui);

            ui.add_space(8.0);
            ui.heading("屏幕布局");
            ui.separator();
            self.draw_layout(ui);

            ui.add_space(8.0);
            ui.heading("邻居");
            ui.separator();
            self.draw_peers(ui);

            ui.add_space(8.0);
            ui.heading("证书与信任");
            ui.separator();
            self.draw_trust(ui);

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("保存").clicked() {
                    if let Err(e) = self.save() {
                        self.notice = format!("保存失败: {e}");
                    }
                }
                if ui.button("收回控制").clicked() {
                    self.poke(status::UiCommand {
                        pause: None,
                        reclaim: true,
                        quit: false,
                    });
                }
                if ui.button("退出 DeskSync").clicked() {
                    self.quit_all(ctx);
                }
            });
            ui.small("点窗口右上角的 × 只是藏到系统托盘，后台继续共享；要彻底退出用上面这个按钮或托盘菜单里的「退出」。");
            if !self.notice.is_empty() {
                ui.colored_label(egui::Color32::from_rgb(40, 120, 90), &self.notice);
            }
        });

        ctx.request_repaint_after(Duration::from_millis(400));
    }
}

impl UiApp {
    fn draw_status(&mut self, ui: &mut egui::Ui) {
        use egui::RichText;
        let alive = status::daemon_alive();
        let (dot, text) = if !alive {
            (egui::Color32::from_rgb(190, 80, 80), "后台服务未运行")
        } else {
            match &self.status {
                Some(s) if s.paused => (egui::Color32::from_rgb(200, 160, 40), "已暂停共享"),
                Some(s) if s.is_forwarding().is_some() => (
                    egui::Color32::from_rgb(40, 150, 100),
                    "正在控制其他机器",
                ),
                Some(s) if s.is_controlled().is_some() => (
                    egui::Color32::from_rgb(60, 130, 200),
                    "正被其他机器控制",
                ),
                Some(s) if !s.connected.is_empty() => {
                    (egui::Color32::from_rgb(40, 150, 100), "已连接，待共享")
                }
                _ => (egui::Color32::from_rgb(150, 150, 150), "运行中，等待邻居"),
            }
        };

        ui.horizontal(|ui| {
            ui.label(RichText::new("●").color(dot).size(18.0));
            ui.label(RichText::new(text).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if alive {
                    if ui.link("停止服务").clicked() {
                        self.poke(status::UiCommand {
                            pause: None,
                            reclaim: false,
                            quit: true,
                        });
                        self.notice = "已通知后台服务退出。".to_string();
                    }
                } else if ui.link("启动服务").clicked() {
                    match spawn_daemon() {
                        Ok(()) => self.notice = "后台服务已启动。".to_string(),
                        Err(e) => self.notice = format!("启动失败: {e}"),
                    }
                }
            });
        });

        // 装了新版但旧后台没死：它占着端口、也听不懂新协议，跨屏必然失效。
        if !self.daemon_version.is_empty() && self.daemon_version != env!("CARGO_PKG_VERSION") {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(200, 70, 70),
                    format!(
                        "后台服务还是 v{}（本程序 v{}）：旧进程没退掉，跨屏不会生效",
                        self.daemon_version,
                        env!("CARGO_PKG_VERSION")
                    ),
                );
                if ui.button("重启后台服务").clicked() {
                    self.restart_daemon();
                }
            });
        }

        // 后台起不来又读不到状态：多半是旧版进程占着端口（安装器关不掉它）。
        if !alive {
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("检查端口是否被旧进程占用").clicked() {
                    self.port_blocked = port_owner(self.cfg.port);
                    self.notice = match self.port_blocked {
                        Some(pid) => format!(
                            "端口 {} 被进程 {pid} 占着（多半是没退干净的旧版后台），可以结束它。",
                            self.cfg.port
                        ),
                        None => format!("端口 {} 没被占用，可以直接启动服务。", self.cfg.port),
                    };
                }
                if let Some(pid) = self.port_blocked {
                    if ui.button("结束它并重启服务").clicked() {
                        let _ = kill_pid(pid);
                        std::thread::sleep(Duration::from_millis(400));
                        match spawn_daemon() {
                            Ok(()) => self.notice = "旧进程已结束，后台服务已重启。".to_string(),
                            Err(e) => self.notice = format!("重启失败: {e}"),
                        }
                        self.port_blocked = None;
                    }
                }
            });
        }

        if let Some(s) = &self.status {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("本机 {} · 端口 {}", s.name, s.port));
                ui.separator();
                ui.label(format!("屏幕 {}x{}", s.screen.0, s.screen.1));
                if !s.connected.is_empty() {
                    ui.separator();
                    ui.label(format!("已连接 {}", s.connected.join("、")));
                }
                let paused_label = if s.paused { "继续共享" } else { "暂停共享" };
                if ui.button(paused_label).clicked() {
                    self.poke(status::UiCommand {
                        pause: Some(!s.paused),
                        reclaim: false,
                        quit: false,
                    });
                }
            });
        }
    }

    fn draw_self(&mut self, ui: &mut egui::Ui) {
        egui::Grid::new("self").num_columns(2).show(ui, |ui| {
            ui.label("本机名称");
            ui.text_edit_singleline(&mut self.cfg.name);
            ui.end_row();
            ui.label("端口");
            let mut port = self.cfg.port.to_string();
            if ui.text_edit_singleline(&mut port).changed() {
                if let Ok(p) = port.parse::<u16>() {
                    self.cfg.port = p;
                }
            }
            ui.end_row();
            ui.label("首次连接自动信任");
            ui.checkbox(&mut self.cfg.auto_trust, "家用局域网建议开启");
            ui.end_row();
        });
    }

    /// 布局画布：所有屏幕按真实分辨率等比摆在虚拟画布上，可自由拖动。
    /// 两台屏幕挨着（有公共边）就能跨屏，光标从公共边的哪个位置出去
    /// 就从对方的对应位置进来——同侧可以摆多台，也可以错位摆放。
    fn draw_layout(&mut self, ui: &mut egui::Ui) {
        let screen = self.screen_size();
        self.cfg.ensure_layout(screen);
        // 草稿和邻居列表保持同步：新邻居给个默认位置（右边），删掉的清掉。
        for p in &self.cfg.peers {
            self.layout_draft
                .entry(p.name.clone())
                .or_insert_with(|| Rect::new(screen.0 as i64, 0, screen.0, screen.1));
        }
        self.layout_draft.retain(|name, _| {
            name == &self.cfg.name || self.cfg.peers.iter().any(|p| &p.name == name)
        });

        let canvas_w = ui.available_width();
        let canvas_h = 230.0;
        let (area, _) = ui.allocate_exact_size(
            egui::vec2(canvas_w, canvas_h),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 8.0, ui.visuals().faint_bg_color);
        painter.rect_stroke(
            area,
            8.0,
            ui.visuals().widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );

        // 包围盒 -> 画布的等比变换
        let mut minx = i64::MAX;
        let mut miny = i64::MAX;
        let mut maxx = i64::MIN;
        let mut maxy = i64::MIN;
        for r in self.layout_draft.values() {
            minx = minx.min(r.left());
            miny = miny.min(r.top());
            maxx = maxx.max(r.right());
            maxy = maxy.max(r.bottom());
        }
        if minx > maxx {
            return;
        }
        let pad = 14.0_f32;
        let bw = (maxx - minx).max(1) as f32;
        let bh = (maxy - miny).max(1) as f32;
        let s = ((canvas_w - pad * 2.0) / bw)
            .min((canvas_h - pad * 2.0) / bh)
            .min(0.2); // 两台机器时别放得太大
        let to_canvas = |vx: i64, vy: i64| -> egui::Pos2 {
            egui::pos2(
                area.left() + pad + (vx - minx) as f32 * s,
                area.top() + pad + (vy - miny) as f32 * s,
            )
        };
        let to_screen_rect = |r: &Rect| -> egui::Rect {
            egui::Rect::from_min_max(to_canvas(r.left(), r.top()), to_canvas(r.right(), r.bottom()))
        };

        let me = *self
            .layout_draft
            .entry(self.cfg.name.clone())
            .or_insert_with(|| Rect::new(0, 0, screen.0, screen.1));

        // 公共边高亮：挨着的屏幕之间画一段亮线
        let mut shared: Vec<(egui::Pos2, egui::Pos2)> = Vec::new();
        for (name, r) in &self.layout_draft {
            if name == &self.cfg.name {
                continue;
            }
            if let Some((edge, seg)) = Rect::shared_border(&me, r, 4) {
                let (a, b) = match edge {
                    Edge::Left => ((r.right(), seg.0), (r.right(), seg.1)),
                    Edge::Right => ((r.left(), seg.0), (r.left(), seg.1)),
                    Edge::Top => ((seg.0, r.bottom()), (seg.1, r.bottom())),
                    Edge::Bottom => ((seg.0, r.top()), (seg.1, r.top())),
                };
                shared.push((to_canvas(a.0, a.1), to_canvas(b.0, b.1)));
            }
        }

        // 先画邻居，再画本机（本机在最上层）
        let mut drag_snap_notice: Option<String> = None;
        for p in self.cfg.peers.clone() {
            let Some(r) = self.layout_draft.get(&p.name).copied() else {
                continue;
            };
            let rect = to_screen_rect(&r);
            let connected = self
                .status
                .as_ref()
                .map(|s| s.connected.iter().any(|c| c == &p.name))
                .unwrap_or(false);
            let fill = if connected {
                egui::Color32::from_rgb(40, 150, 100)
            } else {
                ui.visuals().widgets.inactive.bg_fill
            };
            painter.rect_filled(rect, 5.0, fill);
            painter.rect_stroke(
                rect,
                5.0,
                ui.visuals().widgets.inactive.bg_stroke,
                egui::StrokeKind::Inside,
            );
            let dot = if connected { "● " } else { "○ " };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("{dot}{}\n{}×{}", p.name, r.w, r.h),
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().strong_text_color(),
            );

            // 拖动：虚拟坐标随画布缩放移动，靠近别的屏幕边时自动吸附
            let id = egui::Id::new(("layout_peer", p.name.clone()));
            let resp = ui.interact(rect, id, egui::Sense::drag());
            if resp.dragged() {
                let d = resp.drag_delta();
                let mut nr = r;
                nr.x = (r.x + (d.x / s) as i64).clamp(-4 * screen.0 as i64, 4 * screen.0 as i64);
                nr.y = (r.y + (d.y / s) as i64).clamp(-4 * screen.1 as i64, 4 * screen.1 as i64);
                if let Some(snapped) = snap_rect(&nr, &self.layout_draft, &p.name, 32) {
                    if snapped != nr {
                        drag_snap_notice = Some(p.name.clone());
                    }
                    nr = snapped;
                }
                self.layout_draft.insert(p.name.clone(), nr);
            }
            if resp.hovered() {
                painter.text(
                    rect.left_top() + egui::vec2(0.0, -13.0),
                    egui::Align2::LEFT_BOTTOM,
                    "拖动摆放",
                    egui::TextStyle::Small.resolve(ui.style()),
                    ui.visuals().weak_text_color(),
                );
            }
        }

        // 本机
        let me_rect = to_screen_rect(&me);
        painter.rect_filled(me_rect, 5.0, ui.visuals().widgets.active.bg_fill);
        painter.rect_stroke(
            me_rect,
            5.0,
            ui.visuals().widgets.active.bg_stroke,
            egui::StrokeKind::Inside,
        );
        painter.text(
            me_rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{}\n本机 {}×{}", self.cfg.name, me.w, me.h),
            egui::TextStyle::Body.resolve(ui.style()),
            ui.visuals().strong_text_color(),
        );

        // 公共边最后画，压在色块上
        for (a, b) in shared {
            painter.line_segment(
                [a, b],
                egui::Stroke::new(3.0_f32, egui::Color32::from_rgb(250, 200, 80)),
            );
        }

        if let Some(name) = drag_snap_notice {
            self.notice = format!("{name} 已吸附到相邻位置，保存后生效。");
        }
        ui.small(
            "把邻居拖到任意位置：挨着本机的边（黄线）就能跨屏，同一侧可以放多台、可以错位。\
             屏幕尺寸会按对方真实分辨率自动校正。",
        );
    }

    fn draw_peers(&mut self, ui: &mut egui::Ui) {
        let names: Vec<String> = self.cfg.peers.iter().map(|p| p.name.clone()).collect();
        if names.is_empty() {
            ui.label("还没有添加邻居。");
        }

        let mut remove: Option<String> = None;
        for name in &names {
            let addr = self
                .cfg
                .peers
                .iter()
                .find(|p| &p.name == name)
                .and_then(|p| p.addr.clone())
                .unwrap_or_else(|| "自动发现".to_string());
            let connected = self
                .status
                .as_ref()
                .map(|s| s.connected.iter().any(|c| c == name))
                .unwrap_or(false);

            ui.horizontal(|ui| {
                ui.label(if connected { "●" } else { "○" });
                ui.label(egui::RichText::new(name).strong());
                ui.label(format!("{addr}"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("删除").clicked() {
                        remove = Some(name.clone());
                    }
                    // 位置在「屏幕布局」画布里拖动设置
                });
            });
        }

        if let Some(name) = remove {
            self.cfg.peers.retain(|p| p.name != name);
            self.layout_draft.remove(&name);
            self.notice = format!("已移除 {name}，记得保存。");
        }

        ui.separator();
        ui.label("添加邻居");
        ui.horizontal(|ui| {
            ui.label("名称");
            ui.text_edit_singleline(&mut self.draft.name);
            ui.label("地址");
            ui.text_edit_singleline(&mut self.draft.addr);
            if ui.button("添加").clicked() && !self.draft.name.trim().is_empty() {
                let name = self.draft.name.trim().to_string();
                let addr = self.draft.addr.trim().to_string();
                if self.cfg.peers.iter().any(|p| p.name == name) {
                    self.notice = "已经有同名邻居了。".to_string();
                } else {
                    self.cfg.peers.push(Peer {
                        name: name.clone(),
                        addr: if addr.is_empty() { None } else { Some(addr) },
                    });
                    self.draft = Draft::default();
                    self.notice = "已加入列表，在「屏幕布局」里把它拖到合适的位置后保存。".to_string();
                }
            }
        });
        ui.small("地址形如 192.168.1.20:24987；留空则靠局域网广播发现。");

        // 广播发现到的机器直接点一下就加，不用手打 IP。
        if !self.discovered.is_empty() {
            ui.add_space(6.0);
            ui.label("局域网里发现到的机器");
            let mut add: Option<(String, String)> = None;
            for d in &self.discovered {
                let known = self.cfg.peers.iter().any(|p| p.name == d.name);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&d.name).strong());
                    ui.label(&d.addr);
                    if known {
                        ui.small("（已在邻居列表）");
                    } else if ui.small_button("添加为邻居").clicked() {
                        add = Some((d.name.clone(), d.addr.clone()));
                    }
                });
            }
            if let Some((name, addr)) = add {
                self.cfg.peers.push(Peer {
                    name: name.clone(),
                    addr: Some(addr),
                });
                let screen = self.screen_size();
                self.cfg.ensure_layout(screen);
                self.layout_draft = self.cfg.layout.clone();
                self.notice = format!("已添加 {name}，在「屏幕布局」里把它拖到本机旁边（出现黄线）后保存。");
            }
        } else if status::daemon_alive() {
            ui.small(
                "还没收到邻居的广播。确认两台机器在同一局域网、端口一致，\
                 且防火墙放行了 UDP 24986（发现）与 TCP 24987（数据）。",
            );
        }
    }

    fn draw_trust(&mut self, ui: &mut egui::Ui) {
        if let Some(s) = &self.status {
            ui.horizontal(|ui| {
                ui.label("本机证书指纹");
                let fp = s.fingerprint.clone();
                if ui.button("复制").clicked() {
                    crate::clipboard::copy_text(&fp);
                    self.notice = "指纹已复制到剪贴板。".to_string();
                }
            });
            ui.monospace(short(&s.fingerprint));
        }
        ui.small("两边把对方的指纹加进信任列表，就等于确认了这台机器。");
        if self.cfg.trusted.is_empty() {
            ui.label("暂无已确认的邻居。");
        } else {
            egui::Grid::new("trust").num_columns(2).show(ui, |ui| {
                for (name, fp) in self.cfg.trusted.clone() {
                    ui.label(name);
                    ui.monospace(short(&fp));
                    ui.end_row();
                }
            });
        }
    }
}

fn short(fp: &str) -> String {
    if fp.len() > 24 {
        format!("{}…{}", &fp[..12], &fp[fp.len() - 12..])
    } else {
        fp.to_string()
    }
}

/// 把矩形 `r` 吸附到其他屏幕的边上：某个边在 `tol`（虚拟像素）内就贴齐。
/// 返回吸附后的矩形；不需要吸附时原样返回。
fn snap_rect(
    r: &Rect,
    others: &HashMap<String, Rect>,
    self_name: &str,
    tol: i64,
) -> Option<Rect> {
    let mut nr = *r;
    for (name, o) in others {
        if name == self_name || o == r {
            continue;
        }
        // 水平：贴对方的左边/右边
        if (nr.right() - o.left()).abs() <= tol {
            nr.x = o.left() - nr.w as i64;
        } else if (nr.left() - o.right()).abs() <= tol {
            nr.x = o.right();
        } else if (nr.left() - o.left()).abs() <= tol {
            nr.x = o.x;
        } else if (nr.right() - o.right()).abs() <= tol {
            nr.x = o.right() - nr.w as i64;
        }
        // 垂直：贴对方的顶边/底边
        if (nr.bottom() - o.top()).abs() <= tol {
            nr.y = o.top() - nr.h as i64;
        } else if (nr.top() - o.bottom()).abs() <= tol {
            nr.y = o.bottom();
        } else if (nr.top() - o.top()).abs() <= tol {
            nr.y = o.y;
        } else if (nr.bottom() - o.bottom()).abs() <= tol {
            nr.y = o.bottom() - nr.h as i64;
        }
    }
    if nr == *r {
        None
    } else {
        Some(nr)
    }
}

fn spawn_daemon() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg("run")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

/// 谁占着这个端口（LISTENING 的进程 pid）。旧版后台赖着不退时，
/// status.json 已经读不出来了，只能从端口反查 pid 再动手。
fn port_owner(port: u16) -> Option<u32> {
    let out = if cfg!(windows) {
        std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?
            .stdout
    } else {
        let out = std::process::Command::new("lsof")
            .args(["-ti", &format!("tcp:{port}")])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        return text.lines().filter_map(|l| l.trim().parse::<u32>().ok()).next();
    };

    let text = String::from_utf8_lossy(&out);
    for line in text.lines() {
        // 例：  TCP    0.0.0.0:24987    0.0.0.0:0    LISTENING    12345
        if !line.contains("LISTENING") || !line.contains(&format!(":{port}")) {
            continue;
        }
        if let Some(pid) = line.split_whitespace().last().and_then(|p| p.parse::<u32>().ok()) {
            return Some(pid);
        }
    }
    None
}

/// 按 pid 结束进程。通知后台退出后它还赖着（多半是装新版时旧进程没被杀掉）时的兜底。
fn kill_pid(pid: u32) -> io::Result<()> {
    let me = std::process::id();
    if pid == 0 || pid == me {
        return Ok(());
    }
    crate::log::write(&format!("按 pid 结束进程 {pid}"));
    let pid_s = pid.to_string();
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("taskkill");
        c.args(["/F", "/PID", &pid_s]);
        c
    } else {
        let mut c = std::process::Command::new("kill");
        c.args(["-9", &pid_s]);
        c
    };
    let _ = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(())
}
