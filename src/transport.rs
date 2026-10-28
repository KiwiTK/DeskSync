use crate::identity::Identity;
use crate::identity::fingerprint;
use crate::proto::Frame;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DistinguishedName, ServerConfig, ServerConnection, StreamOwned,
};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// 信任状态：已确认的指纹集合 + 待确认队列。
#[derive(Debug, Default)]
pub struct TrustState {
    pub trusted: HashMap<String, String>,
    pub pending: Vec<String>,
    pub auto_trust: bool,
}

pub type Trust = Arc<Mutex<TrustState>>;

/// 只做证书指纹比对的验证器：不依赖 CA，身份由应用层确认。
#[derive(Debug)]
struct PinVerifier {
    trust: Trust,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl PinVerifier {
    fn check(&self, cert: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let fp = fingerprint(cert.as_ref());
        let mut state = self.trust.lock().unwrap();
        if state.trusted.values().any(|v| *v == fp) {
            return Ok(());
        }
        if state.auto_trust {
            return Ok(());
        }
        if !state.pending.contains(&fp) {
            state.pending.push(fp);
        }
        // 握手阶段放行，是否真的接受由应用层按核对码决定。
        Ok(())
    }

    fn schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

impl rustls::client::danger::ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes()
    }
}

impl rustls::server::danger::ClientCertVerifier for PinVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        self.check(end_entity)?;
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes()
    }
}

fn private_key(id: &Identity) -> PrivateKeyDer<'static> {
    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(id.key_der.clone()))
}

pub fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn server_config(id: &Identity, trust: Trust) -> io::Result<Arc<ServerConfig>> {
    let provider = provider();
    let verifier = Arc::new(PinVerifier {
        trust,
        provider: provider.clone(),
    });
    let certs = vec![CertificateDer::from(id.cert_der.clone())];
    let key = private_key(id);
    ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
        .map(Arc::new)
}

pub fn client_config(id: &Identity, trust: Trust) -> io::Result<Arc<ClientConfig>> {
    let provider = provider();
    let verifier = Arc::new(PinVerifier {
        trust,
        provider: provider.clone(),
    });
    let certs = vec![CertificateDer::from(id.cert_der.clone())];
    let key = private_key(id);
    ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(certs, key)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
        .map(Arc::new)
}

pub type TlsStream = StreamOwned<ServerConnection, TcpStream>;
pub type TlsClient = StreamOwned<ClientConnection, TcpStream>;

pub fn write_frame<W: Write>(w: &mut W, frame: &Frame) -> io::Result<()> {
    let bytes = serde_json::to_vec(frame)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<Frame>> {
    let mut head = [0u8; 4];
    match r.read_exact(&mut head) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(head) as usize;
    if len > 8 * 1024 * 1024 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

pub fn listen(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind(("0.0.0.0", port))
}

/// 已完成握手的连接，暴露给上层统一的读写接口。
pub trait Conn: Read + Write + Send {}

impl Conn for TlsStream {}

impl Conn for TlsClient {}
