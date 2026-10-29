// Windows 上编译成 GUI 子系统：双击不弹黑框。需要终端输出时靠
// attach_parent_console() 附着到父控制台，`desksync status` 在 PowerShell 里照样能打印。
#![cfg_attr(windows, windows_subsystem = "windows")]

mod clipboard;
mod config;
mod cursor;
mod discovery;
mod identity;
mod log;
mod proto;
mod screen;
mod session;
mod status;
mod transport;

#[cfg(feature = "ui")]
mod ui;

use config::Config;
use identity::Identity;
use proto::Frame;
use session::Session;
use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use transport::{TrustState, read_frame, write_frame};

/// 主线程收到的所有消息。
enum Msg {
    /// 本机捕获到的输入。
    Input(rdev::EventType),
    /// 邻居发来的报文。
    Frame(String, Frame),
    /// 新连接建立。附带对方 Hello 里自报的屏幕尺寸。
    Attach(String, Sender<Frame>, Option<(u32, u32)>),
    /// 连接断开。
    Detach(String),
}

fn main() {
    attach_parent_console();

    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let no_input = args.iter().any(|a| a == "--no-input");
    args.retain(|a| a != "--no-input");

    // 日志和 panic 钩子必须最先装上：GUI 进程崩溃时这是唯一的现场。
    log::init();

    let cmd = args.first().cloned().unwrap_or_default();

    let result = match cmd.as_str() {
        "init" => cmd_init(&args),
        "status" => cmd_status(),
        "run" => run(no_input),
        // 不带参数：带了界面的构建打开界面（顺带拉起后台服务），纯命令行构建直接后台跑。
        "" | "ui" => bootstrap(),
        other => {
            eprintln!("未知命令: {other}");
            eprintln!("用法: desksync [init <名字> | status | run | ui] [--no-input]");
            std::process::exit(1);
        }
    };
    if let Err(e) = result {
        log::write(&format!("命令 {cmd:?} 失败: {e}"));
        eprintln!("失败: {e}");
        std::process::exit(1);
    }
}

#[cfg(feature = "ui")]
fn bootstrap() -> std::io::Result<()> {
    ui::run()
}

/// 从终端启动时把 stdout/stderr 接回父控制台，双击启动时静默失败即可。
#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

#[cfg(not(feature = "ui"))]
fn bootstrap() -> std::io::Result<()> {
    run(false)
}

fn cmd_init(args: &[String]) -> std::io::Result<()> {
    let name = args.get(1).cloned().unwrap_or_else(default_name);
    let mut cfg = Config::default();
    cfg.name = name;
    cfg.save()?;
    println!("已写入 {}", Config::path().display());
    println!("打开图形界面（直接运行 desksync）拖动屏幕布局，或在配置文件里填：");
    println!("  peers: [{{\"name\": \"对方名字\", \"addr\": \"192.168.1.20:24987\"}}]");
    Ok(())
}

fn default_name() -> String {
    Config::default().name
}

fn cmd_status() -> std::io::Result<()> {
    let cfg = Config::load().unwrap_or_default();
    let id = Identity::load_or_create(&cfg.name)?;
    println!("名称: {}", cfg.name);
    println!("端口: {}", cfg.port);
    // CI 会校验这一行：打包出来的 exe 必须带界面，否则双击只会静默跑后台。
    println!("界面支持: {}", if cfg!(feature = "ui") { "是" } else { "否" });
    println!("证书指纹: {}", id.fingerprint);
    for (name, r) in &cfg.layout {
        println!("布局: {name} = ({},{}) {}x{}", r.x, r.y, r.w, r.h);
    }
    println!("已信任: {:?}", cfg.trusted.keys().collect::<Vec<_>>());
    println!("配置: {}", Config::path().display());
    Ok(())
}

fn run(no_input: bool) -> std::io::Result<()> {
    let cfg = Config::load().unwrap_or_else(|_| {
        let c = Config::default();
        let _ = c.save();
        c
    });
    let id = Identity::load_or_create(&cfg.name)?;
    // Windows 多显示器：rdev 只报主屏尺寸，鼠标坐标却是整个虚拟桌面的，
    // 直接用会让贴边判定在显示器接缝处乱触发。优先读虚拟桌面矩形。
    let geom = screen::geometry();
    let (w, h) = match cfg.screen {
        Some((sw, sh)) => (sw as f64, sh as f64),
        None => match geom {
            Some(g) => (g.w as f64, g.h as f64),
            None => {
                let (sw, sh) = rdev::display_size().map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::Other, format!("读取屏幕尺寸失败: {e:?}"))
                })?;
                (sw as f64, sh as f64)
            }
        },
    };
    // 虚拟桌面原点（副屏在左/上时坐标为负）：捕获到的坐标减掉它，注入时加回来。
    let (ox, oy) = geom
        .map(|g| (g.x as f64, g.y as f64))
        .unwrap_or((0.0, 0.0));

    let trust = Arc::new(Mutex::new(TrustState {
        trusted: cfg.trusted.clone(),
        pending: Vec::new(),
        auto_trust: cfg.auto_trust,
    }));
    let connected: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    let mut cfg = cfg;
    let screen = (w.round() as u32, h.round() as u32);
    cfg.ensure_layout(screen);
    let mut session = Session::new(w, h, cfg.name.clone(), cfg.layout.clone());
    session.set_origin(ox, oy);
    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();

    println!("DeskSync 已启动");
    println!("  名称 {}  端口 {}  屏幕 {}x{}", cfg.name, cfg.port, w, h);
    if ox != 0.0 || oy != 0.0 {
        crate::log::write(&format!("多显示器：虚拟桌面原点 ({ox}, {oy})，尺寸 {w}x{h}"));
    }
    println!("  证书指纹 {}", id.fingerprint);
    println!("  配置文件 {}", Config::path().display());
    crate::log::write(&format!(
        "后台: 配置与证书就绪 名称={} 端口={} 屏幕={w}x{h} no_input={no_input}",
        cfg.name, cfg.port
    ));

    if no_input {
        println!("  --no-input：不捕获也不注入键鼠");
    } else {
        let tx = msg_tx.clone();
        thread::spawn(move || {
            let result = rdev::listen(move |ev| {
                let _ = tx.send(Msg::Input(ev.event_type));
            });
            if let Err(e) = result {
                let hint = "输入捕获失败，请检查系统权限（辅助功能 / 输入监控）";
                eprintln!("{hint}: {e:?}");
                crate::log::write(&format!("{hint}: {e:?}"));
            }
        });
    }

    // 局域网发现：广播自己，同时收集别人的地址
    let (beacon_tx, beacon_rx) = mpsc::channel::<(String, IpAddr, u16)>();
    if let Err(e) = discovery::spawn(cfg.name.clone(), cfg.port, beacon_tx) {
        eprintln!("局域网发现未启用: {e}");
        crate::log::write(&format!("局域网发现未启用: {e}"));
    }

    // 入站连接
    let listener: TcpListener = transport::listen(cfg.port)?;
    let server_cfg = transport::server_config(&id, trust.clone())?;
    crate::log::write(&format!("后台: 端口 {} 监听就绪", cfg.port));
    let our_name = cfg.name.clone();
    let our_screen = screen;
    let accept_tx = msg_tx.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_nodelay(true);
            // 阻塞写 + 10ms 读超时：避免非阻塞写半包，同时能兼顾出方向延迟。
            let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
            let cfg2 = server_cfg.clone();
            let name = our_name.clone();
            let screen2 = our_screen;
            let tx = accept_tx.clone();
            thread::spawn(move || {
                if let Ok(conn) = rustls::ServerConnection::new(cfg2) {
                    serve(
                        rustls::StreamOwned::new(conn, stream),
                        &name,
                        screen2,
                        &tx,
                    );
                }
            });
        }
    });

    // 出站连接：名字较小的一方发起，避免双方各建一条
    let dial_name = cfg.name.clone();
    let dial_peers: Vec<(String, Option<String>)> = cfg
        .peers
        .iter()
        .map(|p| (p.name.clone(), p.addr.clone()))
        .collect();
    let dial_cfg = transport::client_config(&id, trust.clone())?;
    let dial_tx = msg_tx.clone();
    let dial_connected = connected.clone();
    let dial_screen = screen;
    // 广播发现到的邻居，界面拿去显示「一键添加」。
    let discovered: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let discovered_dial = discovered.clone();
    thread::spawn(move || {
        let mut seen: HashMap<String, (IpAddr, u16)> = HashMap::new();
        for (name, addr) in &dial_peers {
            if let Some(a) = addr {
                if let Some(parsed) = parse_addr(a) {
                    seen.insert(name.clone(), parsed);
                }
            }
        }
        loop {
            while let Ok((name, ip, port)) = beacon_rx.try_recv() {
                seen.insert(name.clone(), (ip, port));
                if let Ok(mut d) = discovered_dial.lock() {
                    d.insert(name, format!("{ip}:{port}"));
                }
            }
            let busy = dial_connected.lock().unwrap().clone();
            for (name, (ip, port)) in seen.iter() {
                if dial_name.as_str() >= name.as_str() || busy.contains(name) {
                    continue;
                }
                let Ok(stream) = TcpStream::connect((*ip, *port)) else {
                    continue;
                };
                let _ = stream.set_nodelay(true);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
                let Ok(conn) = rustls::ClientConnection::new(
                    dial_cfg.clone(),
                    rustls::pki_types::ServerName::from(*ip),
                ) else {
                    continue;
                };
                let tx = dial_tx.clone();
                let n = dial_name.clone();
                let screen2 = dial_screen;
                thread::spawn(move || {
                    serve(
                        rustls::StreamOwned::new(conn, stream),
                        &n,
                        screen2,
                        &tx,
                    );
                });
            }
            thread::sleep(Duration::from_secs(3));
        }
    });

    let mut clipboard = clipboard::Clipboard::new();
    let mut last_clip = Instant::now();
    let mut last_tick = Instant::now();
    let mut cfg_stamp = config_stamp();

    loop {
        match msg_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(msg) => match msg {
                Msg::Input(mut ev) => {
                    // 系统给的鼠标坐标是虚拟桌面的，换算成本机会话坐标（减原点）。
                    if let rdev::EventType::MouseMove { x, y } = &mut ev {
                        *x -= ox;
                        *y -= oy;
                    }
                    session.handle_local(&ev)
                }
                Msg::Frame(peer, frame) => {
                    if let Frame::Clipboard { text } = &frame {
                        clipboard.adopt(text);
                    } else {
                        session.handle_frame(&peer, frame);
                    }
                }
                Msg::Attach(name, tx, peer_screen) => {
                    if session.peers.contains_key(&name) {
                        // 已有同名连接，丢掉新通道让那个线程退出
                        drop(tx);
                    } else {
                        println!("已连接: {name}");
                        connected.lock().unwrap().insert(name.clone());
                        if let Some(s) = peer_screen {
                            session.update_peer_screen(&name, s);
                        }
                        session.connect(name, tx);
                    }
                }
                Msg::Detach(name) => {
                    println!("已断开: {name}");
                    connected.lock().unwrap().remove(&name);
                    session.disconnect(&name);
                }
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // 定期家务：处理界面发来的指令、重载配置、刷新状态文件。
        if last_tick.elapsed() >= Duration::from_millis(300) {
            last_tick = Instant::now();

            if let Some(cmd) = status::take_command() {
                if let Some(p) = cmd.pause {
                    session.paused = p;
                    println!("{}", if p { "已暂停共享" } else { "已继续共享" });
                }
                if cmd.reclaim && session.force_return() {
                    println!("已收回控制");
                }
                if cmd.quit {
                    return Ok(());
                }
            }

            let stamp = config_stamp();
            if stamp != cfg_stamp {
                cfg_stamp = stamp;
                match Config::load() {
                    Ok(mut new_cfg) => {
                        new_cfg.ensure_layout(screen);
                        session.set_layout(new_cfg.layout.clone());
                        println!("配置已重载，无需重启");
                    }
                    Err(e) => eprintln!("配置读取失败，沿用旧配置: {e}"),
                }
            }

            let names: Vec<String> = connected.lock().unwrap().iter().cloned().collect();
            let mut seen: Vec<status::Discovered> = discovered
                .lock()
                .map(|d| {
                    d.iter()
                        .map(|(name, addr)| status::Discovered {
                            name: name.clone(),
                            addr: addr.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            seen.sort_by(|a, b| a.name.cmp(&b.name));
            let _ = status::write_status(&status::Status {
                name: cfg.name.clone(),
                port: cfg.port,
                fingerprint: id.fingerprint.clone(),
                screen,
                paused: session.paused,
                mode: session.mode_str(),
                connected: names,
                written_ms: 0,
                version: env!("CARGO_PKG_VERSION").to_string(),
                pid: std::process::id(),
                discovered: seen,
            });
        }

        if last_clip.elapsed() > Duration::from_millis(400) {
            last_clip = Instant::now();
            let senders = session.senders();
            clipboard.poll(&senders);
        }
    }

    Ok(())
}

/// 配置文件的修改时间，用来判断界面是不是改动过它。
fn config_stamp() -> Option<SystemTime> {
    std::fs::metadata(Config::path())
        .ok()
        .and_then(|m| m.modified().ok())
}

/// 读超时 / 非阻塞 / 中断都当作“暂时没数据”，继续轮询。
fn retryable(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        ErrorKind::WouldBlock | ErrorKind::Interrupted | ErrorKind::TimedOut
    )
}

fn parse_addr(text: &str) -> Option<(IpAddr, u16)> {
    let text = text.trim();
    let (host, port) = match text.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (text, 24987),
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');
    Some((host.parse::<IpAddr>().ok()?, port))
}

/// 一条连接的读写循环：先交换 Hello，然后转发报文。
fn serve<C: transport::Conn>(
    mut stream: C,
    name: &str,
    screen: (u32, u32),
    tx: &Sender<Msg>,
) {
    let mut hello_sent = false;
    let mut peer = None;
    let mut peer_screen = None;
    for _ in 0..4000 {
        if !hello_sent {
            match write_frame(
                &mut stream,
                &Frame::Hello {
                    name: name.to_string(),
                    screen: Some(screen),
                },
            ) {
                Ok(()) => hello_sent = true,
                Err(e) if retryable(&e) => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(_) => return,
            }
        }
        match read_frame(&mut stream) {
            Ok(Some(Frame::Hello { name, screen })) => {
                peer = Some(name);
                peer_screen = screen;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(e) if retryable(&e) => thread::sleep(Duration::from_millis(2)),
            Err(_) => return,
        }
    }
    let Some(peer) = peer else { return };
    let (out_tx, out_rx) = mpsc::channel::<Frame>();
    if tx.send(Msg::Attach(peer.clone(), out_tx, peer_screen)).is_err() {
        return;
    }
    loop {
        let mut closed = false;
        loop {
            match out_rx.try_recv() {
                Ok(frame) => {
                    if write_frame(&mut stream, &frame).is_err() {
                        closed = true;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        if closed {
            break;
        }
        match read_frame(&mut stream) {
            Ok(Some(frame)) => {
                if tx.send(Msg::Frame(peer.clone(), frame)).is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) if retryable(&e) => thread::sleep(Duration::from_millis(1)),
            Err(_) => break,
        }
    }
    let _ = tx.send(Msg::Detach(peer));
}
