//! Indstillinger fra smtp-graph.toml.

use anyhow::{bail, Context, Result};
use base64::Engine;
use ipnet::IpNet;
use serde::Deserialize;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub smtp: SmtpCfg,
    pub graph: GraphCfg,
    #[serde(default)]
    pub koe: KoeCfg,
    /// Mappen, config-filen ligger i. Kø, log og certifikat lægges relativt til den.
    #[serde(skip)]
    pub mappe: PathBuf,
    #[serde(skip)]
    pub fil: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpCfg {
    /// Adresser med almindelig SMTP (STARTTLS tilbydes, hvis `starttls = true`).
    #[serde(default = "std_lyt")]
    pub lyt: Vec<String>,
    /// Adresser med TLS fra start (typisk port 465).
    #[serde(default)]
    pub lyt_tls: Vec<String>,
    /// Kun disse net må forbinde. Tom liste = ingen.
    pub tilladte_ip: Vec<String>,
    #[serde(default = "std_navn")]
    pub vaertsnavn: String,
    #[serde(default = "std_max")]
    pub max_stoerrelse_mb: u64,
    #[serde(default = "sand")]
    pub starttls: bool,
    /// PEM-filer. Mangler de, laves et selvsigneret certifikat i mappen.
    pub tls_cert: Option<PathBuf>,
    pub tls_noegle: Option<PathBuf>,
    /// Hvilken TLS-kode der bruges (auto = rustls, og Windows' Schannel til ældre klienter).
    #[serde(default)]
    pub tls_motor: TlsMotor,
    /// Kræv AUTH, før der må sendes.
    #[serde(default)]
    pub kraev_login: bool,
    /// Tillad AUTH uden TLS (mange scannere kan ikke andet).
    #[serde(default = "sand")]
    pub login_uden_tls: bool,
    #[serde(default)]
    pub bruger: Vec<Bruger>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsMotor {
    /// rustls først; kan klienten ikke forhandle med den, tager Windows (Schannel) over.
    #[default]
    Auto,
    Rustls,
    Windows,
}

impl TlsMotor {
    pub fn navn(self) -> &'static str {
        match self {
            TlsMotor::Auto => "auto",
            TlsMotor::Rustls => "rustls",
            TlsMotor::Windows => "windows",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bruger {
    pub navn: String,
    pub adgangskode: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphCfg {
    /// Opsætningskode fra Opsaet-SmtpGraph.ps1 (IRVMAIL1:…). Erstatter de fem felter herunder.
    pub kode: Option<String>,
    pub tenant: Option<String>,
    pub client_id: Option<String>,
    pub secret: Option<String>,
    /// Postkassen, appen er låst til.
    pub postkasse: Option<String>,
    /// Afsenderadressen (postkassen selv eller en adresse, den må sende som).
    pub from: Option<String>,
    /// Afsendernavn i From-feltet. Exchange bruger ofte postkassens eget navn alligevel.
    pub afsender_navn: Option<String>,
    /// Sæt scannerens egen afsender som Reply-To.
    #[serde(default)]
    pub svar_til_original: bool,
    /// Tilladte modtagere: hele adresser eller domæner ("kunde.dk"). Tom = alle.
    #[serde(default)]
    pub tilladte_modtagere: Vec<String>,
    /// Hvem får besked, når en mail opgives.
    pub fejl_til: Option<String>,
    /// Send ikke rigtigt; gem i koe/toerlob i stedet.
    #[serde(default)]
    pub toerlob: bool,
    /// "app" (standard): kundens egen app med hemmelighed, låst til postkassen.
    /// "bruger": logget ind som en bruger; login-token ligger i `login_fil`.
    pub metode: Option<String>,
    /// Login-token for metode "bruger" (relativ til mappen). Standard graph-login.json.
    pub login_fil: Option<PathBuf>,
    /// Irvolds fælles login-app (multi-tenant), hvis den skal afvige fra den indbyggede.
    pub faelles_app: Option<String>,
    /// Kun til test mod en falsk Graph.
    pub login_url: Option<String>,
    pub graph_url: Option<String>,
    #[serde(skip)]
    pub udloeber: Option<String>,
}

impl GraphCfg {
    pub fn er_bruger(&self) -> bool {
        self.metode.as_deref().is_some_and(|m| m.eq_ignore_ascii_case("bruger"))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KoeCfg {
    #[serde(default = "std_koe")]
    pub mappe: PathBuf,
    /// Opgiv en mail efter så mange timer.
    #[serde(default = "std_timer")]
    pub max_timer: u64,
}

impl Default for KoeCfg {
    fn default() -> Self {
        Self { mappe: std_koe(), max_timer: std_timer() }
    }
}

fn std_lyt() -> Vec<String> { vec!["0.0.0.0:25".into()] }
fn std_navn() -> String { "smtp-graph".into() }
fn std_max() -> u64 { 35 }
fn std_koe() -> PathBuf { "koe".into() }
fn std_timer() -> u64 { 48 }
fn sand() -> bool { true }

/// Den faste del af Graph-opsætningen, når koden er pakket ud.
#[derive(Debug, Clone)]
pub struct GraphAdgang {
    pub tenant: String,
    pub client_id: String,
    pub noegle: Noegle,
    pub postkasse: String,
    pub from: String,
}

#[derive(Debug, Clone)]
pub enum Noegle {
    /// Kundens egen app: client credentials med hemmelighed.
    Hemmelighed(String),
    /// Logget ind som bruger: refresh-token i denne fil (fornyes løbende).
    Login(PathBuf),
}

impl GraphAdgang {
    pub fn konto_tekst(&self) -> String {
        match &self.noegle {
            Noegle::Hemmelighed(_) => format!("app {}", self.client_id),
            Noegle::Login(f) => match crate::oauth::Login::laes(f) {
                Ok(l) => format!("logget ind som {}", l.konto),
                Err(_) => "ikke logget ind".into(),
            },
        }
    }
}

#[derive(Deserialize)]
struct Kode {
    tenant: String,
    client_id: String,
    secret: String,
    postkasse: String,
    from: Option<String>,
    udloeber: Option<String>,
}

impl Config {
    pub fn laes(fil: &Path) -> Result<Config> {
        let tekst = std::fs::read_to_string(fil)
            .with_context(|| format!("kan ikke læse {}", fil.display()))?;
        Self::fra_tekst(&tekst, fil)
    }

    /// Læs og tjek indstillinger fra tekst (`fil` bestemmer kun mappen).
    pub fn fra_tekst(tekst: &str, fil: &Path) -> Result<Config> {
        let mut c: Config = toml::from_str(tekst)
            .with_context(|| format!("fejl i {}", fil.display()))?;
        let fil = std::path::absolute(fil)?;
        c.mappe = fil.parent().map(Path::to_path_buf).unwrap_or_else(|| ".".into());
        c.fil = fil;
        if c.koe.mappe.is_relative() {
            c.koe.mappe = c.mappe.join(&c.koe.mappe);
        }
        c.tjek()?;
        Ok(c)
    }

    fn tjek(&mut self) -> Result<()> {
        for n in &self.smtp.tilladte_ip {
            parse_net(n).with_context(|| format!("tilladte_ip: »{n}« er ikke et net"))?;
        }
        if self.smtp.lyt.is_empty() && self.smtp.lyt_tls.is_empty() {
            bail!("smtp.lyt og smtp.lyt_tls er begge tomme");
        }
        if self.smtp.kraev_login && self.smtp.bruger.is_empty() {
            bail!("kraev_login = true, men der er ingen [[smtp.bruger]]");
        }
        // Graph-adgangen tjekkes først ved start, så SMTP-delen kan sættes op før Microsoft 365.
        if self.graph.kode.as_deref().is_some_and(|k| !k.trim().is_empty()) {
            self.graph_adgang().context("graph.kode")?;
        }
        Ok(())
    }

    pub fn login_fil(&self) -> PathBuf {
        let f = self.graph.login_fil.clone().unwrap_or_else(|| "graph-login.json".into());
        if f.is_relative() { self.mappe.join(f) } else { f }
    }

    pub fn faelles_app(&self) -> String {
        self.graph
            .faelles_app
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| crate::oauth::FAELLES_APP.to_string())
    }

    pub fn graph_adgang(&mut self) -> Result<GraphAdgang> {
        if self.graph.er_bruger() {
            let fil = self.login_fil();
            let l = crate::oauth::Login::laes(&fil)
                .with_context(|| "ikke logget ind med Microsoft endnu (Microsoft 365-fanen i SMTP-Graph)")?;
            let postkasse = self.graph.postkasse.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| l.konto.clone());
            return Ok(GraphAdgang {
                tenant: l.tenant,
                client_id: l.klient,
                noegle: Noegle::Login(fil),
                from: self.graph.from.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| postkasse.clone()),
                postkasse,
            });
        }
        let g = &mut self.graph;
        if let Some(kode) = g.kode.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            let b64 = kode
                .strip_prefix("IRVMAIL1:")
                .or_else(|| kode.strip_prefix("SMTPGRAPH1:"))
                .context("graph.kode skal starte med IRVMAIL1: eller SMTPGRAPH1:")?;
            let json = base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .context("graph.kode er ikke gyldig base64")?;
            let k: Kode = serde_json::from_slice(&json).context("graph.kode kan ikke læses")?;
            g.tenant.get_or_insert(k.tenant);
            g.client_id.get_or_insert(k.client_id);
            g.secret.get_or_insert(k.secret);
            g.from.get_or_insert(k.from.unwrap_or_else(|| k.postkasse.clone()));
            g.postkasse.get_or_insert(k.postkasse);
            g.udloeber = k.udloeber;
        }
        let mangler = |n: &str| anyhow::anyhow!("graph.{n} mangler (eller brug graph.kode)");
        let postkasse = g.postkasse.clone().ok_or_else(|| mangler("postkasse"))?;
        Ok(GraphAdgang {
            tenant: g.tenant.clone().ok_or_else(|| mangler("tenant"))?,
            client_id: g.client_id.clone().ok_or_else(|| mangler("client_id"))?,
            noegle: Noegle::Hemmelighed(g.secret.clone().ok_or_else(|| mangler("secret"))?),
            from: g.from.clone().unwrap_or_else(|| postkasse.clone()),
            postkasse,
        })
    }

    pub fn ip_tilladt(&self, ip: IpAddr) -> bool {
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            v4 => v4,
        };
        self.smtp
            .tilladte_ip
            .iter()
            .filter_map(|n| parse_net(n).ok())
            .any(|n| n.contains(&ip))
    }

    pub fn modtager_tilladt(&self, adr: &str) -> bool {
        let liste = &self.graph.tilladte_modtagere;
        if liste.is_empty() {
            return true;
        }
        let adr = adr.to_ascii_lowercase();
        let domaene = adr.rsplit_once('@').map(|(_, d)| d).unwrap_or("");
        liste.iter().any(|t| {
            let t = t.trim().to_ascii_lowercase();
            let t = t.trim_start_matches('@');
            if t.contains('@') { t == adr } else { t == domaene || domaene.ends_with(&format!(".{t}")) }
        })
    }
}

fn parse_net(s: &str) -> Result<IpNet> {
    let s = s.trim();
    if let Ok(n) = s.parse::<IpNet>() {
        return Ok(n);
    }
    let ip: IpAddr = s.parse()?;
    Ok(IpNet::from(ip))
}

pub const SKABELON: &str = r#"# SMTP-Graph – tager imod mail fra scannere og sender dem gennem Microsoft Graph.

[smtp]
# Almindelig SMTP. Tilføj fx "0.0.0.0:587" hvis scanneren vil have den port.
lyt = ["0.0.0.0:25"]
# TLS fra start (SSL/TLS i scannerens menu), typisk port 465. Tom = slået fra.
lyt_tls = []
# Kun disse net må aflevere mail. Skriv gerne scannerens egen adresse, fx "172.29.3.50".
tilladte_ip = ["127.0.0.1", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"]
vaertsnavn = "smtp-graph"
max_stoerrelse_mb = 35
# STARTTLS tilbydes med et selvsigneret certifikat, hvis tls_cert/tls_noegle ikke er sat.
starttls = true
# tls_cert = "cert.pem"
# tls_noegle = "noegle.pem"
# "auto": moderne TLS (rustls) og Windows' egen TLS til ældre scannere, der ellers
# melder "ingen sikker forbindelse". "rustls" eller "windows" tvinger den ene.
tls_motor = "auto"

# Login er valgfrit. Med kraev_login = true skal scanneren logge ind.
kraev_login = false
login_uden_tls = true
# [[smtp.bruger]]
# navn = "scanner"
# adgangskode = "skift-mig"

[graph]
# Opsætningskoden fra Opsaet-SmtpGraph.ps1:
kode = ""
# … eller felterne hver for sig:
# tenant = ""
# client_id = ""
# secret = ""
# postkasse = "scanner@firma.dk"
# from = "scanner@firma.dk"

# afsender_navn = "Scanner"
svar_til_original = false
# Kun til disse adresser/domæner. Tom = alle.
tilladte_modtagere = []
# Få besked, når en mail ikke kan sendes:
# fejl_til = "it@firma.dk"
# Afprøvning uden at sende: mails gemmes i koe\toerlob.
toerlob = false

[koe]
mappe = "koe"
max_timer = 48
"#;
