//! Reserve: når Microsoft ikke kan sende, sendes mailen gennem en almindelig SMTP-server
//! i stedet – fx Gmail med en app-adgangskode eller udbyderens egen mailserver.
//!
//! Afsenderen skrives om til reserve-kontoen (Gmail tillader kun den), og Bcc-feltet fjernes,
//! så modtagerne ikke ser hinanden. Konvolutmodtagerne (RCPT TO) er de samme som ved Graph.

use crate::graph::Fejl;
use crate::mime::{self, Afsender};
use lettre::address::{Address, Envelope};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReserveTls {
    /// Almindelig forbindelse, der skifter til TLS (typisk port 587).
    #[default]
    Starttls,
    /// TLS fra start (typisk port 465).
    Tls,
    /// Ingen kryptering (kun til en server på det lokale net).
    Ingen,
}

impl ReserveTls {
    pub fn navn(self) -> &'static str {
        match self {
            ReserveTls::Starttls => "starttls",
            ReserveTls::Tls => "tls",
            ReserveTls::Ingen => "ingen",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReserveCfg {
    /// SMTP-server, fx smtp.gmail.com. Tom = ingen reserve.
    #[serde(default)]
    pub server: String,
    #[serde(default = "std_port")]
    pub port: u16,
    #[serde(default)]
    pub tls: ReserveTls,
    #[serde(default)]
    pub bruger: String,
    #[serde(default)]
    pub adgangskode: String,
    /// Afsenderadresse. Tom = brugernavnet.
    #[serde(default)]
    pub fra: String,
    /// Afsendernavn ved reserven. Tom = graph.afsender_navn.
    #[serde(default)]
    pub afsender_navn: Option<String>,
    /// Reserven bruges, når mailen har ventet så mange minutter på Microsoft.
    /// 0 = med det samme, når Microsoft fejler. Afviser Microsoft mailen helt, bruges den altid med det samme.
    #[serde(default = "std_efter")]
    pub efter_minutter: u64,
    /// Kun til test mod en server med selvsigneret certifikat.
    #[serde(default)]
    pub tillad_ugyldigt_certifikat: bool,
}

fn std_port() -> u16 {
    587
}
fn std_efter() -> u64 {
    15
}

impl Default for ReserveCfg {
    fn default() -> Self {
        ReserveCfg {
            server: String::new(),
            port: std_port(),
            tls: ReserveTls::default(),
            bruger: String::new(),
            adgangskode: String::new(),
            fra: String::new(),
            afsender_navn: None,
            efter_minutter: std_efter(),
            tillad_ugyldigt_certifikat: false,
        }
    }
}

impl ReserveCfg {
    pub fn aktiv(&self) -> bool {
        !self.server.trim().is_empty()
    }

    pub fn fra_adresse(&self) -> &str {
        let f = self.fra.trim();
        if f.is_empty() {
            self.bruger.trim()
        } else {
            f
        }
    }

    pub fn er_gmail(&self) -> bool {
        let s = self.server.trim().to_ascii_lowercase();
        s.ends_with("gmail.com") || s.ends_with("googlemail.com")
    }

    /// Kort beskrivelse til log og oversigt.
    pub fn beskrivelse(&self) -> String {
        format!("{}:{} som {}", self.server.trim(), self.port, self.fra_adresse())
    }

    /// Googles app-adgangskoder vises med mellemrum ("abcd efgh ijkl mnop"); de skal væk.
    fn kode(&self) -> String {
        if self.er_gmail() {
            self.adgangskode.chars().filter(|c| !c.is_whitespace()).collect()
        } else {
            self.adgangskode.clone()
        }
    }

    pub fn tjek(&self) -> anyhow::Result<()> {
        if !self.aktiv() {
            return Ok(());
        }
        if self.fra_adresse().parse::<Address>().is_err() {
            anyhow::bail!("reserve.fra (eller reserve.bruger) skal være en mailadresse");
        }
        if self.bruger.trim().is_empty() != self.adgangskode.is_empty() {
            anyhow::bail!("reserve.bruger og reserve.adgangskode skal udfyldes begge to (eller ingen af dem)");
        }
        Ok(())
    }
}

pub struct Reserve {
    cfg: ReserveCfg,
    smtp: AsyncSmtpTransport<Tokio1Executor>,
}

impl Reserve {
    pub fn ny(cfg: &ReserveCfg, vaertsnavn: &str) -> anyhow::Result<Reserve> {
        cfg.tjek()?;
        let server = cfg.server.trim();
        let param = || {
            TlsParameters::builder(server.to_string())
                .dangerous_accept_invalid_certs(cfg.tillad_ugyldigt_certifikat)
                .build_rustls()
        };
        let tls = match cfg.tls {
            ReserveTls::Starttls => Tls::Required(param()?),
            ReserveTls::Tls => Tls::Wrapper(param()?),
            ReserveTls::Ingen => Tls::None,
        };
        let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(server)
            .port(cfg.port)
            .tls(tls)
            .hello_name(ClientId::Domain(vaertsnavn.to_string()))
            .timeout(Some(Duration::from_secs(60)));
        if !cfg.bruger.trim().is_empty() {
            b = b.credentials(Credentials::new(cfg.bruger.trim().to_string(), cfg.kode()));
        }
        Ok(Reserve { cfg: cfg.clone(), smtp: b.build() })
    }

    pub fn cfg(&self) -> &ReserveCfg {
        &self.cfg
    }

    /// Forbind og log ind uden at sende noget.
    pub async fn proev(&self) -> Result<(), Fejl> {
        match self.smtp.test_connection().await {
            Ok(true) => Ok(()),
            Ok(false) => Err(Fejl { midlertidig: true, tekst: format!("reserven {}: serveren svarede ikke", self.cfg.server.trim()) }),
            Err(e) => Err(fejl(&self.cfg, e)),
        }
    }

    pub async fn send(&self, raw: &[u8], env_fra: &str, til: &[String], navn: Option<&str>, svar: bool) -> Result<String, Fejl> {
        let fra = self.cfg.fra_adresse();
        let navn = self.cfg.afsender_navn.as_deref().filter(|n| !n.trim().is_empty()).or(navn);
        let afs = Afsender { from: fra, navn, svar_til_original: svar };
        let mail = mime::uden_felt(&mime::omskriv(raw, env_fra, til, &afs), "bcc");
        let varig = |t: String| Fejl { midlertidig: false, tekst: format!("reserven: {t}") };
        let afsender: Address = fra.parse().map_err(|e| varig(format!("afsender {fra}: {e}")))?;
        let mut modtagere = Vec::new();
        for t in til {
            modtagere.push(t.parse::<Address>().map_err(|e| varig(format!("modtager {t}: {e}")))?);
        }
        let konvolut = Envelope::new(Some(afsender), modtagere).map_err(|e| varig(e.to_string()))?;
        self.smtp
            .send_raw(&konvolut, &mail)
            .await
            .map(|_| format!("reserve {}", self.cfg.server.trim()))
            .map_err(|e| fejl(&self.cfg, e))
    }
}

fn fejl(cfg: &ReserveCfg, e: lettre::transport::smtp::Error) -> Fejl {
    let mut tekst = format!("reserven {}: {e}", cfg.server.trim());
    if let Some(k) = std::error::Error::source(&e) {
        let k = k.to_string();
        if !tekst.contains(&k) {
            tekst.push_str(&format!(" ({k})"));
        }
    }
    if cfg.er_gmail() && tekst.contains("535") {
        tekst.push_str(" – Gmail kræver en app-adgangskode (myaccount.google.com/apppasswords), ikke kontoens almindelige adgangskode");
    }
    Fejl { midlertidig: !e.is_permanent(), tekst }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn gmail_kode_uden_mellemrum() {
        let c = ReserveCfg { server: "smtp.gmail.com".into(), adgangskode: "abcd efgh ijkl mnop".into(), ..Default::default() };
        assert_eq!(c.kode(), "abcdefghijklmnop");
        let c = ReserveCfg { server: "mail.firma.dk".into(), adgangskode: "med mellemrum".into(), ..Default::default() };
        assert_eq!(c.kode(), "med mellemrum");
    }

    #[test]
    fn fra_er_bruger_som_standard() {
        let c = ReserveCfg { server: "smtp.gmail.com".into(), bruger: "a@gmail.com".into(), adgangskode: "x".into(), ..Default::default() };
        assert_eq!(c.fra_adresse(), "a@gmail.com");
        assert!(c.tjek().is_ok());
        let c = ReserveCfg { server: "smtp.gmail.com".into(), bruger: "a@gmail.com".into(), ..Default::default() };
        assert!(c.tjek().is_err());
    }
}
