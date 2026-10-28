mod clipboard;
mod config;
mod cursor;
mod discovery;
mod identity;
mod proto;
mod session;
mod transport;

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
use std::time::{Duration, Instant};
use transport::{TrustState, read_frame, write_frame};

/// 主线程收到的所有消息。
enum Msg {
    /// 本机捕获到的输入。
    Input(rdev::EventType),
    /// 邻居发来的报文。
    Frame(String, Frame),
    /// 新连接建立。
    Attach(String, Sender<Frame>),
    /// 连接断开。
    Detach(String),
}

fn main() {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let no_input = args.iter().any(|a| a == "--no-input");
    args.retain(|a| a != "--no-input");
    let cmd = args.first().cloned().unwrap_or_else(|| "run".to_string());

    let result = match cmd.as_str() {
        "init" => cmd_init(&args),
        "status" => cmd_status(),
        _ => run(no_input),
    };
    if let Err(e) = result {
        eprintln!("失败: {e}");
        std::process::exit(1);
    }
}

fn cmd_init(args: &[String]) -> std::io::Result<()> {
    let name = args.get(1).cloned().unwrap_or_else(default_name);
    let mut cfg = Config::default();
    cfg.name = name;
    cfg.save()?;
    println!("已写入 {}", Config::path().display());
    println!("用编辑器填好这两项后启动：");
    println!("  edges: {{\"对方名字\": \"right\"}}   // 本机哪条边通向对方");
    println!("  peers: [{{\"name\": \"对方名字\", \"addr\": \"192.168.1.20:24872\"}}]");
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
    println!("证书指纹: {}", id.fingerprint);
    println!("屏幕边缘: {:?}", cfg.edges);
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
    let (w, h) = match cfg.screen {
        Some((sw, sh)) => (sw as f64, sh as f64),
        None => {
            let (sw, sh) = rdev::display_size().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::Other, format!("读取屏幕尺寸失败: {e:?}"))
            })?;
            (sw as f64, sh as f64)
        }
    };

    let trust = Arc::new(Mutex::new(TrustState {
        trusted: cfg.trusted.clone(),
        pending: Vec::new(),
        auto_trust: cfg.auto_trust,
    }));
    let connected: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    let mut session = Session::new(w, h, cfg.edges.clone());
    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();

    println!("DeskSync 已启动");
    println!("  名称 {}  端口 {}  屏幕 {}x{}", cfg.name, cfg.port, w, h);
    println!("  证书指纹 {}", id.fingerprint);
    println!("  配置文件 {}", Config::path().display());

    if no_input {
        println!("  --no-input：不捕获也不注入键鼠");
    } else {
        let tx = msg_tx.clone();
        thread::spawn(move || {
            let result = rdev::listen(move |ev| {
                let _ = tx.send(Msg::Input(ev.event_type));
            });
            if let Err(e) = result {
                eprintln!("输入捕获失败，请检查系统权限（辅助功能 / 输入监控）: {e:?}");
            }
        });
    }

    // 局域网发现：广播自己，同时收集别人的地址
    let (beacon_tx, beacon_rx) = mpsc::channel::<(String, IpAddr, u16)>();
    if let Err(e) = discovery::spawn(cfg.name.clone(), cfg.port, beacon_tx) {
        eprintln!("局域网发现未启用: {e}");
    }

    // 入站连接
    let listener: TcpListener = transport::listen(cfg.port)?;
    let server_cfg = transport::server_config(&id, trust.clone())?;
    let our_name = cfg.name.clone();
    let accept_tx = msg_tx.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let _ = stream.set_nodelay(true);
            // 阻塞写 + 10ms 读超时：避免非阻塞写半包，同时能兼顾出方向延迟。
            let _ = stream.set_read_timeout(Some(Duration::from_millis(10)));
            let cfg2 = server_cfg.clone();
            let name = our_name.clone();
            let tx = accept_tx.clone();
            thread::spawn(move || {
                if let Ok(conn) = rustls::ServerConnection::new(cfg2) {
                    serve(rustls::StreamOwned::new(conn, stream), &name, &tx);
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
                seen.insert(name, (ip, port));
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
                thread::spawn(move || {
                    serve(rustls::StreamOwned::new(conn, stream), &n, &tx);
                });
            }
            thread::sleep(Duration::from_secs(3));
        }
    });

    let mut clipboard = clipboard::Clipboard::new();
    let mut last_clip = Instant::now();

    for msg in msg_rx {
        match msg {
            Msg::Input(ev) => session.handle_local(&ev),
            Msg::Frame(peer, frame) => {
                if let Frame::Clipboard { text } = &frame {
                    clipboard.adopt(text);
                } else {
                    session.handle_frame(&peer, frame);
                }
            }
            Msg::Attach(name, tx) => {
                if session.peers.contains_key(&name) {
                    // 已有同名连接，丢掉新通道让那个线程退出
                    drop(tx);
                    continue;
                }
                println!("已连接: {name}");
                connected.lock().unwrap().insert(name.clone());
                session.connect(name, tx);
            }
            Msg::Detach(name) => {
                println!("已断开: {name}");
                connected.lock().unwrap().remove(&name);
                session.disconnect(&name);
            }
        }

        if last_clip.elapsed() > Duration::from_millis(400) {
            last_clip = Instant::now();
            let senders = session.senders();
            clipboard.poll(&senders);
        }
    }

    Ok(())
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
        None => (text, 24872),
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');
    Some((host.parse::<IpAddr>().ok()?, port))
}

/// 一条连接的读写循环：先交换 Hello，然后转发报文。
fn serve<C: transport::Conn>(mut stream: C, name: &str, tx: &Sender<Msg>) {
    let mut hello_sent = false;
    let mut peer = None;
    for _ in 0..4000 {
        if !hello_sent {
            match write_frame(&mut stream, &Frame::Hello { name: name.to_string() }) {
                Ok(()) => hello_sent = true,
                Err(e) if retryable(&e) => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(_) => return,
            }
        }
        match read_frame(&mut stream) {
            Ok(Some(Frame::Hello { name })) => {
                peer = Some(name);
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
    if tx.send(Msg::Attach(peer.clone(), out_tx)).is_err() {
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
