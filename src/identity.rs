use crate::config::data_dir;
use sha2::{Digest, Sha256};
use std::io;
use std::path::PathBuf;

/// 每台机器一份自签证书，指纹即身份。
pub struct Identity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
    pub fingerprint: String,
}

impl Identity {
    pub fn load_or_create(name: &str) -> io::Result<Self> {
        let dir = data_dir();
        std::fs::create_dir_all(&dir)?;
        let cert_path = dir.join("cert.der");
        let key_path = dir.join("key.der");
        if cert_path.exists() && key_path.exists() {
            let cert_der = std::fs::read(&cert_path)?;
            let key_der = std::fs::read(&key_path)?;
            let fingerprint = fingerprint(&cert_der);
            return Ok(Self {
                cert_der,
                key_der,
                fingerprint,
            });
        }
        let created = rcgen::generate_simple_self_signed(vec![name.to_string()])
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
        let cert_der = created.cert.der().to_vec();
        let key_der = created.key_pair.serialize_der();
        std::fs::write(&cert_path, &cert_der)?;
        write_private(&key_path, &key_der)?;
        let fingerprint = fingerprint(&cert_der);
        Ok(Self {
            cert_der,
            key_der,
            fingerprint,
        })
    }

    /// 与对端指纹共同派生的六位核对码。
    #[allow(dead_code)]
    pub fn sas(&self, peer: &str) -> String {
        let (a, b) = if self.fingerprint <= peer.to_string() {
            (self.fingerprint.as_str(), peer)
        } else {
            (peer, self.fingerprint.as_str())
        };
        let mut hasher = Sha256::new();
        hasher.update(a.as_bytes());
        hasher.update(b.as_bytes());
        let digest = hasher.finalize();
        let mut n: u32 = 0;
        for byte in digest.iter().take(4) {
            n = (n << 8) | (*byte as u32);
        }
        format!("{:06}", n % 1_000_000)
    }
}

pub fn fingerprint(cert_der: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cert_der);
    hex(&hasher.finalize())
}

pub fn hex(bytes: &[u8]) -> String {
    const T: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(T[(b >> 4) as usize] as char);
        out.push(T[(b & 0x0f) as usize] as char);
    }
    out
}

fn write_private(path: &PathBuf, der: &[u8]) -> io::Result<()> {
    std::fs::write(path, der)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
