//! En lille SMTP-server (RFC 5321) med STARTTLS, AUTH PLAIN/LOGIN og SIZE.

use crate::config::Config;
use crate::koe::Koe;
use anyhow::Result;
use base64::Engine;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use crate::tls::Tls;

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub struct Faelles {
    pub cfg: Config,
    pub koe: Koe,
    pub tls: Option<Tls>,
    pub ny_post: Notify,
}

const TIMEOUT: Duration = Duration::from_secs(300);
const MAX_LINJE: usize = 64 * 1024;
const MAX_MODTAGERE: usize = 100;

pub async fn lyt(f: Arc<Faelles>, adr: String, tls_fra_start: bool) -> Result<()> {
    let l = TcpListener::bind(&adr).await.map_err(|e| anyhow::anyhow!("kan ikke lytte på {adr}: {e}"))?;
    tracing::info!("lytter på {adr}{}", if tls_fra_start { " (TLS)" } else { "" });
    loop {
        let (sock, peer) = match l.accept().await {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("accept: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let f = f.clone();
        tokio::spawn(async move {
            if !f.cfg.ip_tilladt(peer.ip()) {
                tracing::warn!("{peer}: afvist (ikke i tilladte_ip)");
                let mut s = sock;
                let _ = s.write_all(b"554 5.7.1 Adgang naegtet\r\n").await;
                return;
            }
            let _ = sock.set_nodelay(true);
            let r = if tls_fra_start {
                match &f.tls {
                    Some(acc) => match tokio::time::timeout(TIMEOUT, acc.accept(Box::new(sock), peer)).await {
                        Ok(Ok(t)) => session(&f, t, peer, true).await,
                        Ok(Err(e)) => Err(e),
                        Err(_) => Err(anyhow::anyhow!("TLS: tidsudløb")),
                    },
                    None => Err(anyhow::anyhow!("TLS er ikke sat op")),
                }
            } else {
                session(&f, Box::new(sock), peer, false).await
            };
            if let Err(e) = r {
                tracing::info!("{peer}: forbindelsen sluttede: {e:#}");
            }
        });
    }
}

enum Slut {
    Faerdig,
    StartTls(Box<dyn Io>),
}

async fn session(f: &Faelles, io: Box<dyn Io>, peer: SocketAddr, tls: bool) -> Result<()> {
    let mut s = Session::new(f, peer, tls);
    let mut rd = BufReader::new(io);
    s.svar(&mut rd, &format!("220 {} ESMTP SMTP-Graph", f.cfg.smtp.vaertsnavn)).await?;
    let r = s.koer(&mut rd).await;
    let mut spor = std::mem::take(&mut s.spor);
    let mut mails = s.mails;
    let r = match r {
        Ok(Slut::StartTls(raa)) => {
            let acc = f.tls.as_ref().expect("STARTTLS kun når TLS findes");
            let t = tokio::time::timeout(TIMEOUT, acc.accept(raa, peer))
                .await
                .map_err(|_| anyhow::anyhow!("TLS: tidsudløb"))??;
            spor.push("(TLS)".into());
            let mut s = Session::new(f, peer, true);
            let mut rd = BufReader::new(t);
            let r = s.koer(&mut rd).await;
            spor.append(&mut s.spor);
            mails += s.mails;
            r.map(|_| ())
        }
        r => r.map(|_| ()),
    };
    if mails == 0 && spor.iter().any(|k| !k.starts_with("QUIT")) {
        let fejl = r.as_ref().err().map(|e| format!(" – {e:#}")).unwrap_or_default();
        tracing::info!("{peer}: ingen mail i forbindelsen ({}){fejl}", spor.join(", "));
        return Ok(());
    }
    r
}

struct Session<'a> {
    f: &'a Faelles,
    peer: SocketAddr,
    tls: bool,
    helo: Option<String>,
    bruger: Option<String>,
    fra: Option<String>,
    til: Vec<String>,
    fejl: u32,
    /// Kommandoer og svarkoder (uden data/adgangskoder) til loggen, hvis intet blev sendt.
    spor: Vec<String>,
    mails: u32,
}

impl<'a> Session<'a> {
    fn new(f: &'a Faelles, peer: SocketAddr, tls: bool) -> Self {
        Session { f, peer, tls, helo: None, bruger: None, fra: None, til: vec![], fejl: 0, spor: vec![], mails: 0 }
    }

    fn max(&self) -> usize { (self.f.cfg.smtp.max_stoerrelse_mb * 1024 * 1024) as usize }

    fn auth_mulig(&self) -> bool {
        !self.f.cfg.smtp.bruger.is_empty() && (self.tls || self.f.cfg.smtp.login_uden_tls)
    }

    async fn svar(&mut self, rd: &mut BufReader<Box<dyn Io>>, linje: &str) -> Result<()> {
        if let Some(sidste) = self.spor.last_mut() {
            if !sidste.contains(' ') {
                sidste.push(' ');
                sidste.push_str(linje.get(..3).unwrap_or(linje));
            }
        }
        let w = rd.get_mut();
        // Hele svaret i én skrivning (= én TLS-pakke). HP PageWide går i stå efter 354,
        // hvis linjeskiftet kommer i sin egen pakke.
        let mut b = Vec::with_capacity(linje.len() + 2);
        b.extend_from_slice(linje.as_bytes());
        b.extend_from_slice(b"\r\n");
        w.write_all(&b).await?;
        w.flush().await?;
        Ok(())
    }

    async fn laes_linje(&mut self, rd: &mut BufReader<Box<dyn Io>>) -> Result<Option<String>> {
        let mut buf = Vec::new();
        let n = tokio::time::timeout(TIMEOUT, (&mut *rd).take(MAX_LINJE as u64).read_until(b'\n', &mut buf))
            .await
            .map_err(|_| anyhow::anyhow!("tidsudløb"))??;
        if n == 0 {
            return Ok(None);
        }
        if !buf.ends_with(b"\n") {
            anyhow::bail!("for lang linje");
        }
        while buf.last().is_some_and(|c| *c == b'\n' || *c == b'\r') {
            buf.pop();
        }
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }

    fn nulstil(&mut self) {
        self.fra = None;
        self.til.clear();
    }

    async fn koer(&mut self, rd: &mut BufReader<Box<dyn Io>>) -> Result<Slut> {
        loop {
            let Some(linje) = self.laes_linje(rd).await? else { return Ok(Slut::Faerdig) };
            let (kmd, arg) = match linje.split_once(' ') {
                Some((k, a)) => (k.to_ascii_uppercase(), a.trim().to_string()),
                None => (linje.trim().to_ascii_uppercase(), String::new()),
            };
            if self.spor.len() < 40 {
                // Kun kommandoens navn (og AUTH-metoden) – aldrig adresser eller adgangskoder.
                let mut navn: String = kmd.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
                if kmd == "AUTH" {
                    navn.push('-');
                    navn.extend(arg.split(' ').next().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric()).take(12));
                }
                if navn.is_empty() {
                    navn = "?".into();
                }
                self.spor.push(navn);
            }
            let ok = match kmd.as_str() {
                "EHLO" | "HELO" => {
                    self.helo = Some(arg.clone());
                    self.nulstil();
                    if kmd == "HELO" {
                        self.svar(rd, &format!("250 {}", self.f.cfg.smtp.vaertsnavn)).await?;
                    } else {
                        let mut l = vec![
                            format!("{} hej {}", self.f.cfg.smtp.vaertsnavn, self.peer.ip()),
                            format!("SIZE {}", self.max()),
                            "8BITMIME".into(),
                            "PIPELINING".into(),
                            "ENHANCEDSTATUSCODES".into(),
                        ];
                        if !self.tls && self.f.tls.is_some() && self.f.cfg.smtp.starttls {
                            l.push("STARTTLS".into());
                        }
                        if self.auth_mulig() {
                            l.push("AUTH PLAIN LOGIN".into());
                        }
                        let n = l.len();
                        let alle: Vec<String> = l
                            .into_iter()
                            .enumerate()
                            .map(|(i, t)| format!("250{}{t}", if i + 1 == n { ' ' } else { '-' }))
                            .collect();
                        self.svar(rd, &alle.join("\r\n")).await?;
                    }
                    true
                }
                "STARTTLS" => {
                    if self.tls || self.f.tls.is_none() || !self.f.cfg.smtp.starttls {
                        self.svar(rd, "502 5.5.1 STARTTLS findes ikke her").await?;
                        false
                    } else {
                        self.svar(rd, "220 2.0.0 Klar til TLS").await?;
                        // Buffer smides væk med vilje (beskytter mod indsprøjtede kommandoer).
                        let (tom, _) = tokio::io::duplex(1);
                        let raa = std::mem::replace(rd, BufReader::new(Box::new(tom) as Box<dyn Io>));
                        return Ok(Slut::StartTls(raa.into_inner()));
                    }
                }
                "AUTH" => self.auth(rd, &arg).await?,
                "MAIL" => self.mail(rd, &arg).await?,
                "RCPT" => self.rcpt(rd, &arg).await?,
                "DATA" => self.data(rd).await?,
                "BDAT" => {
                    self.svar(rd, "502 5.5.1 BDAT understøttes ikke").await?;
                    false
                }
                "RSET" => {
                    self.nulstil();
                    self.svar(rd, "250 2.0.0 OK").await?;
                    true
                }
                "NOOP" => {
                    self.svar(rd, "250 2.0.0 OK").await?;
                    true
                }
                "VRFY" => {
                    self.svar(rd, "252 2.5.0 Proever at levere").await?;
                    true
                }
                "HELP" => {
                    self.svar(rd, "214 2.0.0 EHLO MAIL RCPT DATA RSET NOOP QUIT").await?;
                    true
                }
                "QUIT" => {
                    self.svar(rd, "221 2.0.0 Farvel").await?;
                    return Ok(Slut::Faerdig);
                }
                "" => false,
                _ => {
                    self.svar(rd, "500 5.5.2 Ukendt kommando").await?;
                    false
                }
            };
            if !ok {
                self.fejl += 1;
                if self.fejl >= 20 {
                    self.svar(rd, "421 4.7.0 For mange fejl").await?;
                    return Ok(Slut::Faerdig);
                }
            }
        }
    }

    async fn auth(&mut self, rd: &mut BufReader<Box<dyn Io>>, arg: &str) -> Result<bool> {
        if !self.auth_mulig() {
            self.svar(rd, "503 5.5.1 AUTH er ikke slået til").await?;
            return Ok(false);
        }
        if self.bruger.is_some() {
            self.svar(rd, "503 5.5.1 Allerede logget ind").await?;
            return Ok(false);
        }
        let mut dele = arg.split_whitespace();
        let mek = dele.next().unwrap_or("").to_ascii_uppercase();
        let start = dele.next().map(str::to_string);
        let b64 = base64::engine::general_purpose::STANDARD;
        let (navn, kode) = match mek.as_str() {
            "PLAIN" => {
                let svar = match start {
                    Some(s) if s != "=" => s,
                    _ => {
                        self.svar(rd, "334 ").await?;
                        self.laes_linje(rd).await?.unwrap_or_default()
                    }
                };
                if svar == "*" {
                    self.svar(rd, "501 5.0.0 Afbrudt").await?;
                    return Ok(false);
                }
                let raa = b64.decode(svar.trim()).unwrap_or_default();
                let d: Vec<&[u8]> = raa.split(|c| *c == 0).collect();
                if d.len() != 3 {
                    self.svar(rd, "501 5.5.2 Ugyldigt PLAIN-svar").await?;
                    return Ok(false);
                }
                (String::from_utf8_lossy(d[1]).into_owned(), String::from_utf8_lossy(d[2]).into_owned())
            }
            "LOGIN" => {
                let n = match start {
                    Some(s) => s,
                    None => {
                        self.svar(rd, "334 VXNlcm5hbWU6").await?;
                        self.laes_linje(rd).await?.unwrap_or_default()
                    }
                };
                self.svar(rd, "334 UGFzc3dvcmQ6").await?;
                let k = self.laes_linje(rd).await?.unwrap_or_default();
                let dek = |s: &str| String::from_utf8_lossy(&b64.decode(s.trim()).unwrap_or_default()).into_owned();
                (dek(&n), dek(&k))
            }
            _ => {
                self.svar(rd, "504 5.5.4 Kun PLAIN og LOGIN").await?;
                return Ok(false);
            }
        };
        let ok = self.f.cfg.smtp.bruger.iter().any(|b| b.navn.eq_ignore_ascii_case(&navn) && konst_lig(&b.adgangskode, &kode));
        if ok {
            tracing::info!("{}: logget ind som {navn}", self.peer);
            self.bruger = Some(navn);
            self.svar(rd, "235 2.7.0 Logget ind").await?;
            Ok(true)
        } else {
            tracing::warn!("{}: forkert login for »{navn}«", self.peer);
            tokio::time::sleep(Duration::from_secs(2)).await;
            self.svar(rd, "535 5.7.8 Forkert brugernavn eller adgangskode").await?;
            Ok(false)
        }
    }

    async fn mail(&mut self, rd: &mut BufReader<Box<dyn Io>>, arg: &str) -> Result<bool> {
        if self.f.cfg.smtp.kraev_login && self.bruger.is_none() {
            self.svar(rd, "530 5.7.0 Log ind først").await?;
            return Ok(false);
        }
        if self.fra.is_some() {
            self.svar(rd, "503 5.5.1 MAIL er allerede givet").await?;
            return Ok(false);
        }
        let Some((adr, param)) = sti(arg, "FROM:") else {
            self.svar(rd, "501 5.5.4 Brug: MAIL FROM:<adresse>").await?;
            return Ok(false);
        };
        for p in param.split_whitespace() {
            if let Some(v) = p.to_ascii_uppercase().strip_prefix("SIZE=") {
                if v.parse::<usize>().unwrap_or(0) > self.max() {
                    self.svar(rd, "552 5.3.4 Mailen er for stor").await?;
                    return Ok(false);
                }
            }
        }
        self.fra = Some(adr);
        self.svar(rd, "250 2.1.0 OK").await?;
        Ok(true)
    }

    async fn rcpt(&mut self, rd: &mut BufReader<Box<dyn Io>>, arg: &str) -> Result<bool> {
        if self.fra.is_none() {
            self.svar(rd, "503 5.5.1 MAIL først").await?;
            return Ok(false);
        }
        let Some((adr, _)) = sti(arg, "TO:") else {
            self.svar(rd, "501 5.5.4 Brug: RCPT TO:<adresse>").await?;
            return Ok(false);
        };
        if !adr.contains('@') {
            self.svar(rd, "553 5.1.3 Ugyldig adresse").await?;
            return Ok(false);
        }
        if !self.f.cfg.modtager_tilladt(&adr) {
            tracing::warn!("{}: modtager {adr} er ikke tilladt", self.peer);
            self.svar(rd, "550 5.7.1 Modtageren er ikke tilladt").await?;
            return Ok(false);
        }
        if self.til.len() >= MAX_MODTAGERE {
            self.svar(rd, "452 4.5.3 For mange modtagere").await?;
            return Ok(false);
        }
        if !self.til.iter().any(|t| t.eq_ignore_ascii_case(&adr)) {
            self.til.push(adr);
        }
        self.svar(rd, "250 2.1.5 OK").await?;
        Ok(true)
    }

    async fn data(&mut self, rd: &mut BufReader<Box<dyn Io>>) -> Result<bool> {
        if self.fra.is_none() || self.til.is_empty() {
            self.svar(rd, "503 5.5.1 MAIL og RCPT først").await?;
            return Ok(false);
        }
        self.svar(rd, "354 Send mailen; slut med <CRLF>.<CRLF>").await?;
        let max = self.max();
        let mut data: Vec<u8> = Vec::with_capacity(64 * 1024);
        let mut for_stor = false;
        let mut linje = Vec::new();
        // Lange linjer læses i bidder; kun en rigtig linjestart kan være "." eller dot-stuffing.
        let mut linjestart = true;
        loop {
            linje.clear();
            let n = tokio::time::timeout(TIMEOUT, (&mut *rd).take(1024 * 1024).read_until(b'\n', &mut linje))
                .await
                .map_err(|_| {
                    let hale: String = data[data.len().saturating_sub(12)..]
                        .iter()
                        .chain(linje.iter())
                        .flat_map(|b| std::ascii::escape_default(*b))
                        .map(char::from)
                        .collect();
                    anyhow::anyhow!("tidsudløb under DATA ({} bytes modtaget, slutter med \"{hale}\")", data.len() + linje.len())
                })??;
            if n == 0 {
                anyhow::bail!("forbindelsen lukket under DATA");
            }
            let start = linjestart;
            linjestart = linje.ends_with(b"\n");
            if start && (linje == b".\r\n" || linje == b".\n") {
                break;
            }
            let indhold: &[u8] = if start && linje.starts_with(b".") { &linje[1..] } else { &linje };
            if for_stor || data.len() + indhold.len() > max {
                for_stor = true;
                continue;
            }
            data.extend_from_slice(indhold);
            // Normalisér nøgne LF til CRLF.
            if data.ends_with(b"\n") && !data.ends_with(b"\r\n") {
                data.pop();
                data.extend_from_slice(b"\r\n");
            }
        }
        let fra = self.fra.take().unwrap_or_default();
        let til = std::mem::take(&mut self.til);
        if for_stor {
            tracing::warn!("{}: mail fra {fra} var over {} MB", self.peer, self.f.cfg.smtp.max_stoerrelse_mb);
            self.svar(rd, "552 5.3.4 Mailen er for stor").await?;
            return Ok(false);
        }
        match self.f.koe.laeg(&fra, &til, &self.peer.to_string(), &data).await {
            Ok(post) => {
                tracing::info!(
                    "{}: modtog {} ({} kB) fra {fra} til {}",
                    self.peer,
                    post.id,
                    data.len() / 1024,
                    til.join(", ")
                );
                self.f.ny_post.notify_one();
                self.mails += 1;
                self.svar(rd, &format!("250 2.0.0 OK, i koe som {}", post.id)).await?;
                Ok(true)
            }
            Err(e) => {
                tracing::error!("kan ikke lægge i kø: {e:#}");
                self.svar(rd, "451 4.3.0 Midlertidig fejl, prøv igen").await?;
                Ok(false)
            }
        }
    }
}

/// Læser "FROM:<a@b> SIZE=1" → ("a@b", "SIZE=1"). Tom afsender (<>) er tilladt.
fn sti(arg: &str, praefiks: &str) -> Option<(String, String)> {
    let a = arg.trim();
    if a.len() < praefiks.len() || !a[..praefiks.len()].eq_ignore_ascii_case(praefiks) {
        return None;
    }
    let rest = a[praefiks.len()..].trim_start();
    if let Some(r) = rest.strip_prefix('<') {
        let slut = r.find('>')?;
        let adr = r[..slut].trim();
        // Kildeangivelse "@a,@b:bruger@x" smides væk.
        let adr = adr.rsplit_once(':').map(|(_, a)| a).unwrap_or(adr);
        Some((adr.to_string(), r[slut + 1..].trim().to_string()))
    } else {
        let mut d = rest.splitn(2, ' ');
        Some((d.next()?.to_string(), d.next().unwrap_or("").to_string()))
    }
}

fn konst_lig(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod test {
    use super::sti;
    #[test]
    fn stier() {
        assert_eq!(sti("FROM:<a@b.dk> SIZE=10", "FROM:"), Some(("a@b.dk".into(), "SIZE=10".into())));
        assert_eq!(sti("from: <>", "FROM:"), Some(("".into(), "".into())));
        assert_eq!(sti("TO:a@b.dk", "TO:"), Some(("a@b.dk".into(), "".into())));
        assert_eq!(sti("TO:<@x.dk:a@b.dk>", "TO:"), Some(("a@b.dk".into(), "".into())));
    }
}
