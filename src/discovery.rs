use crate::proto::Beacon;
use std::net::{IpAddr, UdpSocket};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

/// 每两秒广播一次自己，同时监听别人的广播。
pub fn spawn(name: String, port: u16, tx: Sender<(String, IpAddr, u16)>) -> std::io::Result<()> {
    // 发现端口固定在服务端口前一位，测试时可以整体挪开避免冲突。
    let dport = port.saturating_sub(1);
    let sock = UdpSocket::bind(("0.0.0.0", dport))?;
    sock.set_broadcast(true)?;
    sock.set_read_timeout(Some(Duration::from_millis(400)))?;
    thread::spawn(move || {
        let mut last = Instant::now() - Duration::from_secs(5);
        let mut buf = [0u8; 1024];
        loop {
            if last.elapsed() >= Duration::from_secs(2) {
                let beacon = Beacon {
                    name: name.clone(),
                    port,
                };
                if let Ok(bytes) = serde_json::to_vec(&beacon) {
                    let _ = sock.send_to(&bytes, ("255.255.255.255", dport));
                }
                last = Instant::now();
            }
            if let Ok((n, addr)) = sock.recv_from(&mut buf) {
                if let Ok(beacon) = serde_json::from_slice::<Beacon>(&buf[..n]) {
                    if beacon.name != name {
                        let _ = tx.send((beacon.name, addr.ip(), beacon.port));
                    }
                }
            }
        }
    });
    Ok(())
}
