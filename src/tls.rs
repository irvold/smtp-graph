//! TLS til SMTP-serveren.
//!
//! rustls dækker moderne klienter (TLS 1.2/1.3 med ECDHE). Mange scannere og printere
//! tilbyder kun ældre nøgleudveksling (RSA eller DHE, ingen fælles kurver), og så afviser
//! rustls med fx `NoKxGroupsInCommon`. Med `tls_motor = "auto"` læses klientens ClientHello
//! først; kan rustls ikke forhandle med den, gives forbindelsen (med de læste bytes) videre
//! til Windows' egen TLS (Schannel), som kan det, Windows tillader.

use crate::config::{Config, TlsMotor};
use crate::smtp::Io;
use anyhow::{bail, Context, Result};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskCx, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

pub struct Tls {
    motor: TlsMotor,
    rustls: Option<(Arc<rustls::ServerConfig>, tokio_rustls::TlsAcceptor)>,
    #[cfg(windows)]
    windows: Option<tokio_native_tls::TlsAcceptor>,
}

/// Selvsignerede filer, der laves automatisk (RSA, så også gamle klienter kan bruge dem).
pub fn auto_filer(mappe: &Path) -> (PathBuf, PathBuf) {
    (mappe.join("cert-rsa.pem"), mappe.join("noegle-rsa.pem"))
}

impl Tls {
    pub fn ny(cfg: &Config) -> Result<Option<Tls>> {
        if !cfg.smtp.starttls && cfg.smtp.lyt_tls.is_empty() {
            return Ok(None);
        }
        let motor = cfg.smtp.tls_motor;
        #[cfg(not(windows))]
        if motor == TlsMotor::Windows {
            bail!("tls_motor = \"windows\" virker kun på Windows");
        }
        let rel = |p: &PathBuf| if p.is_relative() { cfg.mappe.join(p) } else { p.clone() };
        let (cert_fil, noegle_fil) = match (&cfg.smtp.tls_cert, &cfg.smtp.tls_noegle) {
            (Some(c), Some(n)) => (rel(c), rel(n)),
            (None, None) => {
                let (c, n) = auto_filer(&cfg.mappe);
                if !c.exists() || !n.exists() {
                    lav_selvsigneret(&cfg.smtp.vaertsnavn, &c, &n)?;
                    tracing::info!("lavede selvsigneret RSA-certifikat {}", c.display());
                }
                (c, n)
            }
            _ => bail!("tls_cert og tls_noegle skal sættes sammen"),
        };
        let cert_pem = std::fs::read(&cert_fil).with_context(|| format!("{}", cert_fil.display()))?;
        let noegle_pem = std::fs::read(&noegle_fil).with_context(|| format!("{}", noegle_fil.display()))?;

        let rustls = if motor == TlsMotor::Windows {
            None
        } else {
            let sc = Arc::new(rustls_cfg(&cert_pem, &noegle_pem)?);
            Some((sc.clone(), tokio_rustls::TlsAcceptor::from(sc)))
        };

        #[cfg(windows)]
        let windows = if motor == TlsMotor::Rustls {
            None
        } else {
            match windows_acceptor(&cert_pem, &noegle_pem) {
                Ok(a) => Some(a),
                Err(e) if motor == TlsMotor::Auto => {
                    tracing::warn!("Windows-TLS kunne ikke startes, kun rustls bruges: {e:#}");
                    None
                }
                Err(e) => return Err(e.context("Windows-TLS")),
            }
        };
        Ok(Some(Tls {
            motor,
            rustls,
            #[cfg(windows)]
            windows,
        }))
    }

    /// TLS-håndtryk på en forbindelse (efter STARTTLS eller fra start).
    pub async fn accept(&self, io: Box<dyn Io>, peer: SocketAddr) -> Result<Box<dyn Io>> {
        #[cfg(windows)]
        let win = self.windows.as_ref();
        #[cfg(not(windows))]
        let win: Option<&()> = None;

        let Some((sc, acc)) = &self.rustls else {
            return self.windows(io, peer).await;
        };
        if win.is_none() || self.motor == TlsMotor::Rustls {
            return Ok(Box::new(acc.accept(io).await.context("TLS")?));
        }

        // Auto: læs ClientHello og se, om rustls kan forhandle med klienten.
        let (io, foer, svar) = laes_hilsen(io).await?;
        let grund = match svar {
            Ok(a) => {
                let beskr = beskriv(&a.client_hello());
                match a.into_connection(sc.clone()) {
                    Ok(_) => None,
                    Err((e, _)) => Some(format!("{e}; klienten tilbyder {beskr}")),
                }
            }
            Err(e) => Some(e),
        };
        match grund {
            None => {
                let io = Box::new(Foran::ny(foer, io));
                Ok(Box::new(acc.accept(io as Box<dyn Io>).await.context("TLS")?))
            }
            Some(g) => {
                tracing::info!("{peer}: ældre TLS, Windows tager over ({g})");
                let ch = hex(&foer[..foer.len().min(1024)]);
                let io = Foran::ny(foer, io);
                let spion = io.spion.clone();
                spion.lock().unwrap().peer = Some(peer);
                let r = self.windows(Box::new(io), peer).await;
                if let Err(e) = &r {
                    let sp = spion.lock().unwrap();
                    tracing::info!(
                        "{peer}: Windows-håndtryk: serveren sendte {} bytes ({}), klienten {} bytes efter ClientHello{}",
                        sp.skrevet_ialt,
                        server_hello(&sp.skrevet),
                        sp.laest.len(),
                        if sp.laest.is_empty() { String::new() } else { format!(" ({})", hex(&sp.laest[..sp.laest.len().min(64)])) }
                    );
                    tracing::info!("{peer}: ClientHello {ch}");
                    let _ = e;
                }
                r
            }
        }
    }

    #[cfg(windows)]
    async fn windows(&self, io: Box<dyn Io>, _peer: SocketAddr) -> Result<Box<dyn Io>> {
        let Some(w) = &self.windows else { bail!("TLS er ikke sat op") };
        match tokio::time::timeout(std::time::Duration::from_secs(30), w.accept(io)).await {
            Err(_) => bail!("TLS via Windows: klienten gik i stå under håndtrykket (30 s)"),
            Ok(Ok(t)) => Ok(Box::new(t)),
            Ok(Err(e)) => bail!(
                "TLS via Windows: {e} (er klientens TLS-version eller kryptering slået fra i Windows, \
                 fx TLS 1.0/1.1, kan scanneren kun bruges uden kryptering)"
            ),
        }
    }

    #[cfg(not(windows))]
    async fn windows(&self, _io: Box<dyn Io>, _peer: SocketAddr) -> Result<Box<dyn Io>> {
        bail!("TLS er ikke sat op")
    }

    pub fn motor_tekst(&self) -> String {
        #[cfg(windows)]
        let w = self.windows.is_some();
        #[cfg(not(windows))]
        let w = false;
        match (self.rustls.is_some(), w) {
            (true, true) => "rustls + Windows (Schannel) til ældre klienter".into(),
            (true, false) => "rustls".into(),
            (false, true) => "Windows (Schannel)".into(),
            (false, false) => "ingen".into(),
        }
    }
}

type Hilsen = std::result::Result<rustls::server::Accepted, String>;

/// Læser indtil rustls har hele ClientHello. Returnerer de læste bytes, så håndtrykket
/// kan spilles igen for den motor, der skal bruges.
async fn laes_hilsen(mut io: Box<dyn Io>) -> Result<(Box<dyn Io>, Vec<u8>, Hilsen)> {
    let mut acc = rustls::server::Acceptor::default();
    let mut foer = Vec::new();
    let mut b = [0u8; 4096];
    loop {
        let n = io.read(&mut b).await.context("TLS-hilsen")?;
        if n == 0 {
            bail!("forbindelsen lukkede under TLS-hilsenen");
        }
        foer.extend_from_slice(&b[..n]);
        let mut r = &b[..n];
        while !r.is_empty() {
            if let Err(e) = acc.read_tls(&mut r) {
                return Ok((io, foer, Err(format!("ClientHello kan ikke læses: {e}"))));
            }
        }
        match acc.accept() {
            Ok(Some(a)) => return Ok((io, foer, Ok(a))),
            Ok(None) if foer.len() < 64 * 1024 => continue,
            Ok(None) => bail!("ClientHello er for stor"),
            Err((e, _)) => return Ok((io, foer, Err(format!("{e}")))),
        }
    }
}

fn beskriv(h: &rustls::server::ClientHello) -> String {
    let suites = h.cipher_suites();
    let mut s: Vec<String> = suites.iter().take(4).map(|c| format!("{c:?}")).collect();
    if suites.len() > 4 {
        s.push(format!("… ({} i alt)", suites.len()));
    }
    let grupper = match h.named_groups() {
        Some(g) if !g.is_empty() => g.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(", "),
        _ => "ingen".into(),
    };
    format!("kurver: {grupper}; krypteringer: {}", s.join(", "))
}

fn rustls_cfg(cert_pem: &[u8], noegle_pem: &[u8]) -> Result<rustls::ServerConfig> {
    let certs = rustls_pemfile::certs(&mut &cert_pem[..]).collect::<Result<Vec<_>, _>>()?;
    let noegle = rustls_pemfile::private_key(&mut &noegle_pem[..])?.context("ingen privat nøgle i nøglefilen")?;
    Ok(rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, noegle)?)
}

#[cfg(windows)]
fn windows_acceptor(cert_pem: &[u8], noegle_pem: &[u8]) -> Result<tokio_native_tls::TlsAcceptor> {
    // Schannel vil have en RSA-nøgle i PKCS#8; PKCS#1 ("RSA PRIVATE KEY") omsættes.
    let tekst = std::str::from_utf8(noegle_pem).context("nøglefilen er ikke PEM")?;
    let pkcs8 = if tekst.contains("BEGIN RSA PRIVATE KEY") {
        use rsa::pkcs1::DecodeRsaPrivateKey;
        use rsa::pkcs8::{EncodePrivateKey, LineEnding};
        rsa::RsaPrivateKey::from_pkcs1_pem(tekst)?.to_pkcs8_pem(LineEnding::LF)?.to_string()
    } else if tekst.contains("BEGIN PRIVATE KEY") {
        tekst.to_string()
    } else {
        bail!("nøglen skal være RSA (PKCS#8 eller PKCS#1) for at Windows kan bruge den");
    };
    let id = native_tls::Identity::from_pkcs8(cert_pem, pkcs8.as_bytes()).context("certifikat til Windows")?;
    let acc = native_tls::TlsAcceptor::builder(id)
        // Lad Windows' egne indstillinger afgøre, hvad der er tilladt (gamle scannere).
        .min_protocol_version(Some(native_tls::Protocol::Tlsv10))
        .build()?;
    Ok(tokio_native_tls::TlsAcceptor::from(acc))
}

fn lav_selvsigneret(vaert: &str, cert: &Path, noegle: &Path) -> Result<()> {
    use chrono::Datelike;
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).context("RSA-nøgle")?;
    let pem = key.to_pkcs8_pem(LineEnding::LF)?;
    let kp = rcgen::KeyPair::from_pem_and_sign_algo(&pem, &rcgen::PKCS_RSA_SHA256)?;
    let mut p = rcgen::CertificateParams::new(vec![vaert.to_string(), "localhost".into()])?;
    p.distinguished_name.push(rcgen::DnType::CommonName, vaert);
    let aar = chrono::Utc::now().year();
    p.not_before = rcgen::date_time_ymd(aar - 1, 1, 1);
    p.not_after = rcgen::date_time_ymd(aar + 20, 1, 1);
    let c = p.self_signed(&kp)?;
    std::fs::write(noegle, pem.as_bytes())?;
    std::fs::write(cert, c.pem())?;
    Ok(())
}

/// Bytes, der allerede er læst, efterfulgt af resten af forbindelsen.
struct Foran {
    buf: Vec<u8>,
    pos: usize,
    io: Box<dyn Io>,
    /// De første bytes hver vej under håndtrykket (til fejlfinding).
    spion: Arc<std::sync::Mutex<Spion>>,
}

#[derive(Default)]
struct Spion {
    skrevet: Vec<u8>,
    skrevet_ialt: usize,
    laest: Vec<u8>,
    laest_ialt: usize,
    /// Sat for Windows-forbindelser: logges, når forbindelsen lukkes.
    peer: Option<SocketAddr>,
}

impl Foran {
    fn ny(buf: Vec<u8>, io: Box<dyn Io>) -> Foran {
        Foran { buf, pos: 0, io, spion: Default::default() }
    }
}

impl Drop for Foran {
    fn drop(&mut self) {
        if let Ok(sp) = self.spion.lock() {
            if let Some(peer) = sp.peer {
                tracing::debug!(
                    "{peer}: Windows-TLS lukket: {} bytes ind og {} bytes ud på nettet efter ClientHello",
                    sp.laest_ialt,
                    sp.skrevet_ialt
                );
            }
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Kort beskrivelse af serverens første svar (ServerHello: version og kryptering).
fn server_hello(b: &[u8]) -> String {
    if b.len() < 44 || b[0] != 0x16 || b[5] != 0x02 {
        return format!("ikke en ServerHello: {}", hex(&b[..b.len().min(16)]));
    }
    let ver = u16::from_be_bytes([b[9], b[10]]);
    let sid = b[43] as usize;
    let suite = b.get(44 + sid..46 + sid).map(|x| u16::from_be_bytes([x[0], x[1]]));
    let ver_navn = match ver {
        0x0301 => "TLS 1.0",
        0x0302 => "TLS 1.1",
        0x0303 => "TLS 1.2",
        _ => "?",
    };
    match suite {
        Some(s) => format!("ServerHello {ver_navn}, kryptering 0x{s:04x}"),
        None => format!("ServerHello {ver_navn}"),
    }
}

impl AsyncRead for Foran {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut TaskCx<'_>, out: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        if self.pos < self.buf.len() {
            let n = out.remaining().min(self.buf.len() - self.pos);
            let pos = self.pos;
            out.put_slice(&self.buf[pos..pos + n]);
            self.pos += n;
            if self.pos == self.buf.len() {
                self.buf = Vec::new();
                self.pos = 0;
            }
            return Poll::Ready(Ok(()));
        }
        let foer = out.filled().len();
        let r = Pin::new(&mut self.io).poll_read(cx, out);
        if let Poll::Ready(Ok(())) = &r {
            let nye = &out.filled()[foer..];
            let mut sp = self.spion.lock().unwrap();
            sp.laest_ialt += nye.len();
            if sp.laest.len() < 256 {
                let n = nye.len().min(256 - sp.laest.len());
                sp.laest.extend_from_slice(&nye[..n]);
            }
        }
        r
    }
}

impl AsyncWrite for Foran {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut TaskCx<'_>, b: &[u8]) -> Poll<std::io::Result<usize>> {
        let r = Pin::new(&mut self.io).poll_write(cx, b);
        if let Poll::Ready(Ok(n)) = &r {
            let mut sp = self.spion.lock().unwrap();
            sp.skrevet_ialt += n;
            if sp.skrevet.len() < 256 {
                let k = (*n).min(256 - sp.skrevet.len());
                sp.skrevet.extend_from_slice(&b[..k]);
            }
        }
        r
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskCx<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskCx<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
