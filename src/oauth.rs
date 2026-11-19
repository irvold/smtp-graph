//! Login med Microsoft i browseren (authorization code + PKCE) gennem Irvolds fælles,
//! multi-tenant app. Bruges til (1) administrator-opsætning hos kunden og
//! (2) metoden "bruger", hvor SMTP-Graph sender som den bruger, der loggede ind.

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Irvolds fælles app (multi-tenant, offentlig klient). Oprettes én gang med Opret-FaellesApp.ps1.
/// Kan overstyres med `graph.faelles_app` i indstillingerne.
pub const FAELLES_APP: &str = "2e5b6c6b-1fd3-4baa-85b2-e487b8ca7512";

pub const LOGIN: &str = "https://login.microsoftonline.com";

/// Rettigheder ved administrator-opsætning.
/// `.default` beder om alle de rettigheder, der er registreret på den fælles app (Graph og Exchange)
/// i ét samtykke. Microsoft tillader ikke rettigheder til to ressourcer hver for sig i samme login.
pub const SCOPE_ADMIN_GRAPH: &str = "openid profile offline_access https://graph.microsoft.com/.default";
pub const SCOPE_ADMIN_EXO: &str = "https://outlook.office365.com/.default";
/// Rettigheder, når der sendes som bruger.
pub const SCOPE_BRUGER: &str = "openid profile offline_access https://graph.microsoft.com/User.Read https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/Mail.ReadWrite.Shared";

#[derive(Debug, Clone)]
pub struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    pub tenant: String,
    pub konto: String,
    pub navn: String,
    pub udloeber_sek: u64,
}

/// Gemt login for metoden "bruger".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Login {
    pub klient: String,
    pub tenant: String,
    pub konto: String,
    pub refresh_token: String,
    pub fornyet: chrono::DateTime<chrono::Utc>,
}

impl Login {
    pub fn laes(fil: &Path) -> Result<Login> {
        let b = std::fs::read(fil).with_context(|| format!("{} findes ikke", fil.display()))?;
        Ok(serde_json::from_slice(&b)?)
    }

    /// Skriv filen. Findes den, overskrives den på stedet, så dens adgangsrettigheder bevares.
    pub fn gem(&self, fil: &Path) -> Result<()> {
        let ny = !fil.exists();
        std::fs::write(fil, serde_json::to_vec_pretty(self)?)?;
        #[cfg(windows)]
        if ny {
            // Kun SYSTEM og administratorer må læse token.
            let _ = std::process::Command::new("icacls")
                .arg(fil)
                .args(["/inheritance:r", "/grant:r", "*S-1-5-18:F", "*S-1-5-32-544:F"])
                .output();
        }
        let _ = ny;
        Ok(())
    }
}

static INDSAET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Kommandolinjen på Linux: login-adressen vises, og adressen, browseren ender på, kan
/// indsættes i terminalen. Så kan man logge ind fra en anden pc end serveren.
pub fn tillad_indsaet() {
    INDSAET.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Hent koden fra den adresse, browseren endte på (http://127.0.0.1:port/?code=…&state=…).
fn kode_fra_adresse(adresse: &str, state: &str) -> Result<String> {
    let a = adresse.trim();
    let u = reqwest::Url::parse(a).or_else(|_| reqwest::Url::parse(&format!("http://127.0.0.1/{}", a.trim_start_matches('/'))))?;
    let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
    if let Some(e) = q.get("error") {
        bail!("Login afbrudt: {e} – {}", q.get("error_description").cloned().unwrap_or_default());
    }
    let kode = q.get("code").context("adressen indeholder ingen code= – kopiér hele adressen fra browserens adresselinje")?;
    if q.get("state").map(String::as_str) != Some(state) {
        bail!("adressen hører ikke til dette login (state passer ikke) – prøv igen");
    }
    Ok(kode.clone())
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn claims(id_token: &str) -> Value {
    id_token
        .split('.')
        .nth(1)
        .and_then(|p| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}

fn aabn_browser(url: &str) -> Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn()?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(60)).build().expect("http")
}

async fn token_svar(svar: reqwest::Response) -> Result<Tokens> {
    let status = svar.status();
    let v: Value = svar.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let fejl = v["error"].as_str().unwrap_or("");
        let besked = v["error_description"].as_str().unwrap_or("ukendt fejl");
        let besked = besked.lines().next().unwrap_or(besked);
        bail!("{fejl}: {besked}");
    }
    let c = claims(v["id_token"].as_str().unwrap_or(""));
    let access = v["access_token"].as_str().context("intet access_token")?.to_string();
    // Uden id_token (ved fornyelse) læses tenant fra selve adgangstokenet.
    let ac = if c.is_null() { claims(&access) } else { c };
    Ok(Tokens {
        refresh: v["refresh_token"].as_str().map(str::to_string),
        tenant: ac["tid"].as_str().unwrap_or("").to_string(),
        konto: ac["preferred_username"].as_str().or(ac["upn"].as_str()).unwrap_or("").to_string(),
        navn: ac["name"].as_str().unwrap_or("").to_string(),
        udloeber_sek: v["expires_in"].as_u64().unwrap_or(3600),
        access,
    })
}

/// Log ind i browseren. `scope` giver tokenet; `ekstra` godkendes samtidig (fx Exchange),
/// så de kan hentes bagefter med [`forny`] uden et nyt login.
pub async fn browser_login(klient: &str, scope: &str, ekstra: &str, vis: &(dyn Fn(String) + Sync)) -> Result<Tokens> {
    if klient.trim().is_empty() {
        bail!("Irvolds fælles login-app er ikke sat (kør Opret-FaellesApp.ps1 én gang, og indsæt app-id'et under Avanceret)");
    }
    let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = uuid::Uuid::new_v4().simple().to_string();
    let lytter = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let redirect = format!("http://127.0.0.1:{}", lytter.local_addr()?.port());
    let alle = if ekstra.is_empty() { scope.to_string() } else { format!("{scope} {ekstra}") };
    let url = reqwest::Url::parse_with_params(
        &format!("{LOGIN}/organizations/oauth2/v2.0/authorize"),
        &[
            ("client_id", klient),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("response_mode", "query"),
            ("scope", alle.as_str()),
            ("state", state.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ],
    )?;
    let kode = if INDSAET.load(std::sync::atomic::Ordering::Relaxed) {
        let skaerm = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        if skaerm {
            let _ = aabn_browser(url.as_str());
        }
        vis(format!(
            "Åbn denne adresse i en browser (det må gerne være på en anden pc):

{url}

             Log ind med Microsoft-kontoen. Til sidst ender browseren på en side, der ikke kan vises
             ({redirect}/?code=…). Kopiér hele adressen fra adresselinjen, indsæt den her og tryk Enter:"
        ));
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        std::thread::spawn(move || {
            let mut linje = String::new();
            if std::io::stdin().read_line(&mut linje).is_ok() {
                let _ = tx.send(linje);
            }
        });
        let st = state.clone();
        tokio::select! {
            k = modtag_kode(lytter, state) => k?,
            l = rx => kode_fra_adresse(&l.context("ingen adresse indtastet")?, &st)?,
        }
    } else {
        vis("Browseren åbner nu. Log ind med Microsoft-kontoen.".into());
        aabn_browser(url.as_str())?;
        modtag_kode(lytter, state).await?
    };

    vis("Login modtaget, henter adgang …".into());
    let svar = http()
        .post(format!("{LOGIN}/organizations/oauth2/v2.0/token"))
        .form(&[
            ("client_id", klient),
            ("grant_type", "authorization_code"),
            ("code", kode.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("code_verifier", verifier.as_str()),
            ("scope", scope),
        ])
        .send()
        .await?;
    token_svar(svar).await
}

/// Hent et nyt adgangstoken (evt. til en anden ressource) med et refresh-token.
pub async fn forny(klient: &str, tenant: &str, refresh: &str, scope: &str, login_url: Option<&str>) -> Result<Tokens> {
    let base = login_url.unwrap_or(LOGIN);
    let svar = http()
        .post(format!("{base}/{tenant}/oauth2/v2.0/token"))
        .form(&[
            ("client_id", klient),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("scope", scope),
        ])
        .send()
        .await?;
    token_svar(svar).await
}

/// Venter på, at browseren kommer tilbage til http://127.0.0.1:port/?code=…&state=….
async fn modtag_kode(lytter: tokio::net::TcpListener, state: String) -> Result<String> {
    tokio::time::timeout(Duration::from_secs(600), async {
        loop {
            let (mut s, _) = lytter.accept().await?;
            let mut buf = vec![0u8; 16384];
            let n = s.read(&mut buf).await?;
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let sti = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let u = reqwest::Url::parse(&format!("http://127.0.0.1{sti}"))?;
            let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
            if q.is_empty() {
                // Fx favicon – ignorér.
                let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            }
            let (ok, tekst) = match (q.get("code"), q.get("error")) {
                (Some(c), _) if q.get("state") == Some(&state) => (Some(c.clone()), "Du er logget ind. Du kan lukke denne fane og gå tilbage til SMTP-Graph.".to_string()),
                (_, Some(e)) => (None, format!("Login afbrudt: {e} – {}", q.get("error_description").cloned().unwrap_or_default())),
                _ => (None, "Ugyldigt svar (state passer ikke).".to_string()),
            };
            let html = format!(
                "<!doctype html><meta charset=utf-8><title>SMTP-Graph</title><body style=\"font-family:Segoe UI,sans-serif;padding:40px\"><h2>SMTP-Graph</h2><p>{}</p></body>",
                tekst.replace('<', "&lt;")
            );
            let _ = s
                .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len()).as_bytes())
                .await;
            return match ok {
                Some(c) => Ok::<String, anyhow::Error>(c),
                None => bail!("{tekst}"),
            };
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("login tog mere end 10 minutter – prøv igen"))?
}

#[cfg(test)]
mod test {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn kald(port: u16, sti: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("GET {sti} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).await.unwrap();
        let mut ud = String::new();
        s.read_to_string(&mut ud).await.unwrap();
        ud
    }

    #[tokio::test]
    async fn modtager_kode_og_afviser_forkert_state() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(modtag_kode(l, "abc".into()));
        let svar = kald(port, "/favicon.ico").await;
        assert!(svar.starts_with("HTTP/1.1 404"), "{svar}");
        let svar = kald(port, "/?code=KODE%2B1&state=abc&session_state=x").await;
        assert!(svar.contains("Du er logget ind"), "{svar}");
        assert_eq!(h.await.unwrap().unwrap(), "KODE+1");

        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let h = tokio::spawn(modtag_kode(l, "abc".into()));
        kald(port, "/?error=access_denied&error_description=Brugeren%20afviste&state=abc").await;
        let e = h.await.unwrap().unwrap_err().to_string();
        assert!(e.contains("access_denied") && e.contains("Brugeren afviste"), "{e}");
    }

    #[test]
    fn kode_fra_indsat_adresse() {
        assert_eq!(kode_fra_adresse("  http://127.0.0.1:5000/?code=K%2B1&state=abc&session_state=x
", "abc").unwrap(), "K+1");
        assert_eq!(kode_fra_adresse("/?code=K&state=abc", "abc").unwrap(), "K");
        assert!(kode_fra_adresse("http://127.0.0.1:5000/?code=K&state=xyz", "abc").is_err());
        assert!(kode_fra_adresse("http://127.0.0.1:5000/?error=access_denied&state=abc", "abc").unwrap_err().to_string().contains("access_denied"));
        assert!(kode_fra_adresse("noget andet", "abc").is_err());
    }

    #[test]
    fn claims_fra_jwt() {
        let krop = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"tid":"t1","preferred_username":"a@b.dk"}"#);
        let c = claims(&format!("x.{krop}.y"));
        assert_eq!(c["tid"], "t1");
        assert_eq!(c["preferred_username"], "a@b.dk");
    }
}
