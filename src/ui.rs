//! 图形界面：菜单栏图标 + 设置窗口。
//!
//! 界面是单独一层，不直接参与键鼠捕获和网络收发——它只读 `status.json`、
//! 写 `config.json` 和 `command.json`，后台服务负责其余部分。
//! 这样界面崩了、被关掉了，共享本身都不受影响。

use crate::config::{Config, Edge, Peer};
use crate::status;
use eframe::egui;
use std::io;
use std::time::{Duration, Instant};

const ICON_PNG: &[u8] = include_bytes!("../assets/icon-128.png");

pub fn run() -> io::Result<()> {
    let cfg = Config::load().unwrap_or_default();

    // 后台服务没在跑就先拉起来，界面退出后它继续干活。
    if !status::daemon_alive() {
        spawn_daemon()?;
        std::thread::sleep(Duration::from_millis(700));
    }

    // 托盘不能在这里建：macOS 的 NSStatusItem 要等 NSApplication 起来之后才能创建，
    // 在 run_native 之前碰它会直接崩在 CGSConnectionByID。所以推迟到第一帧。
    eframe::run_native(
        "DeskSync",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([580.0, 560.0])
                .with_min_inner_size([480.0, 420.0])
                .with_icon(load_icon()),
            ..Default::default()
        },
        Box::new(move |_cc| Ok(Box::new(UiApp::new(cfg)))),
    )
    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
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
    edge: usize,
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
}

const EDGE_LABELS: [&str; 4] = ["左", "右", "上", "下"];

fn edge_of(i: usize) -> Edge {
    match i {
        0 => Edge::Left,
        1 => Edge::Right,
        2 => Edge::Top,
        _ => Edge::Bottom,
    }
}

fn index_of(e: Edge) -> usize {
    match e {
        Edge::Left => 0,
        Edge::Right => 1,
        Edge::Top => 2,
        Edge::Bottom => 3,
    }
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
        }
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
        self.cfg.save()?;
        self.notice = "已保存，屏幕方向立即生效；改名或换端口需要重启后台服务。".to_string();
        Ok(())
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
        while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if ev.id == *tray.open.id() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if ev.id == *tray.pause.id() {
                let paused = self.status.as_ref().map(|s| s.paused).unwrap_or(false);
                self.poke(status::UiCommand {
                    pause: Some(!paused),
                    reclaim: false,
                    quit: false,
                });
            } else if ev.id == *tray.reclaim.id() {
                self.poke(status::UiCommand {
                    pause: None,
                    reclaim: true,
                    quit: false,
                });
            } else if ev.id == *tray.quit.id() {
                self.poke(status::UiCommand {
                    pause: None,
                    reclaim: false,
                    quit: true,
                });
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        let _ = tray_icon::TrayIconEvent::receiver().try_recv();
    }
}

impl eframe::App for UiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_tray();
        self.handle_tray_events(ctx);

        if self.last_poll.elapsed() > Duration::from_millis(400) {
            self.last_poll = Instant::now();
            self.status = status::read_status();
        }

        // 点关闭只是藏到菜单栏，真正退出走托盘菜单里的「退出」。
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("DeskSync");
            ui.separator();

            self.draw_status(ui);

            ui.add_space(8.0);
            egui::CollapsingHeader::new("本机")
                .default_open(true)
                .show(ui, |ui| self.draw_self(ui));
            egui::CollapsingHeader::new("屏幕布局与邻居")
                .default_open(true)
                .show(ui, |ui| self.draw_peers(ui));
            egui::CollapsingHeader::new("证书与信任")
                .default_open(false)
                .show(ui, |ui| self.draw_trust(ui));

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
            });
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
            let edge_idx = self
                .cfg
                .edges
                .get(name)
                .copied()
                .map(index_of)
                .unwrap_or(1);
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
                    let mut idx = edge_idx;
                    egui::ComboBox::from_id_salt(format!("edge_{name}"))
                        .width(70.0)
                        .selected_text(EDGE_LABELS[idx])
                        .show_ui(ui, |ui| {
                            for (i, label) in EDGE_LABELS.iter().enumerate() {
                                ui.selectable_value(&mut idx, i, *label);
                            }
                        });
                    ui.label("通向它的边");
                    if idx != edge_idx {
                        self.cfg.edges.insert(name.clone(), edge_of(idx));
                    }
                });
            });
        }

        if let Some(name) = remove {
            self.cfg.peers.retain(|p| p.name != name);
            self.cfg.edges.remove(&name);
            self.notice = format!("已移除 {name}，记得保存。");
        }

        ui.separator();
        ui.label("添加邻居");
        ui.horizontal(|ui| {
            ui.label("名称");
            ui.text_edit_singleline(&mut self.draft.name);
            ui.label("地址");
            ui.text_edit_singleline(&mut self.draft.addr);
        });
        ui.horizontal(|ui| {
            ui.label("出口方向");
            egui::ComboBox::from_id_salt("draft_edge")
                .selected_text(EDGE_LABELS[self.draft.edge])
                .show_ui(ui, |ui| {
                    for (i, label) in EDGE_LABELS.iter().enumerate() {
                        ui.selectable_value(&mut self.draft.edge, i, *label);
                    }
                });
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
                    self.cfg.edges.insert(name, edge_of(self.draft.edge));
                    self.draft = Draft::default();
                    self.notice = "已加入列表，记得保存。".to_string();
                }
            }
        });
        ui.small("地址形如 192.168.1.20:24872；留空则靠局域网广播发现。");
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
