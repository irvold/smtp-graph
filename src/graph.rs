//! Microsoft Graph: token (client credentials) og afsendelse.

use crate::config::GraphAdgang;
use crate::mime::{self, Afsender};
use base64::Engine;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Rå MIME over denne grænse sendes i stedet som kladde + upload (Graph tager højst 4 MB pr. kald,
/// og base64 fylder 4/3).
pub const MIME_GRAENSE: usize = 2_900_000;
/// Vedhæftninger over denne størrelse uploades i bidder.
const LILLE_VEDH: usize = 2_900_000;
const BID: usize = 3 * 1024 * 1024;

#[derive(Debug)]
pub struct Fejl {
    pub midlertidig: bool,
    pub tekst: String,
}

impl std::fmt::Display for Fejl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.tekst)
    }
}

fn midl(t: impl Into<String>) -> Fejl { Fejl { midlertidig: true, tekst: t.into() } }

pub struct Graph {
    adgang: GraphAdgang,
    http: reqwest::Client,
    login_url: String,
    graph_url: String,
    token: Mutex<Option<(String, Instant)>>,
}

impl Graph {
    pub fn ny(adgang: GraphAdgang, login_url: Option<String>, graph_url: Option<String>) -> Graph {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(20))
            .user_agent(concat!("SMTP-Graph/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http-klient");
        Graph {
            adgang,
            http,
            login_url: login_url.unwrap_or_else(|| "https://login.microsoftonline.com".into()),
            graph_url: graph_url.unwrap_or_else(|| "https://graph.microsoft.com/v1.0".into()),
            token: Mutex::new(None),
        }
    }

    pub async fn token(&self) -> Result<String, Fejl> {
        let mut t = self.token.lock().await;
        if let Some((tok, udl)) = t.as_ref() {
            if Instant::now() < *udl {
                return Ok(tok.clone());
            }
        }
        let secret = match &self.adgang.noegle {
            crate::config::Noegle::Hemmelighed(s) => s.clone(),
            crate::config::Noegle::Login(fil) => {
                let (tok, sek) = self.forny_login(fil).await?;
                *t = Some((tok.clone(), Instant::now() + Duration::from_secs(sek.saturating_sub(300).max(60))));
                return Ok(tok);
            }
        };
        let url = format!("{}/{}/oauth2/v2.0/token", self.login_url, self.adgang.tenant);
        let svar = self
            .http
            .post(&url)
            .form(&[
                ("client_id", self.adgang.client_id.as_str()),
                ("client_secret", secret.as_str()),
                ("scope", "https://graph.microsoft.com/.default"),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await
            .map_err(|e| midl(format!("login: {e}")))?;
        let status = svar.status();
        let v: Value = svar.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let besked = v["error_description"].as_str().or(v["error"].as_str()).unwrap_or("ukendt");
            let besked = besked.lines().next().unwrap_or(besked);
            // Forkert hemmelighed/app bliver ikke bedre af at vente, men hold mailen i køen alligevel,
            // så den kan sendes, når opsætningen er rettet.
            return Err(midl(format!("login afvist ({status}): {besked}")));
        }
        let tok = v["access_token"].as_str().ok_or_else(|| midl("login: intet access_token"))?.to_string();
        let sek = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(300).max(60);
        *t = Some((tok.clone(), Instant::now() + Duration::from_secs(sek)));
        Ok(tok)
    }

    async fn glem_token(&self) {
        *self.token.lock().await = None;
    }

    /// Metoden "bruger": hent adgangstoken med det gemte refresh-token, og gem det nye
    /// refresh-token (Microsoft udskifter det ved hver fornyelse).
    async fn forny_login(&self, fil: &std::path::Path) -> Result<(String, u64), Fejl> {
        let mut l = crate::oauth::Login::laes(fil).map_err(|e| midl(format!("login: {e:#}")))?;
        let login_url = (self.login_url != crate::oauth::LOGIN).then_some(self.login_url.as_str());
        let t = crate::oauth::forny(&l.klient, &l.tenant, &l.refresh_token, "https://graph.microsoft.com/.default offline_access", login_url)
            .await
            .map_err(|e| {
                let s = format!("{e:#}");
                if s.contains("invalid_grant") || s.contains("interaction_required") {
                    midl(format!("login udløbet for {} – log ind igen i SMTP-Graph (Microsoft 365-fanen): {s}", l.konto))
                } else {
                    midl(format!("login: {s}"))
                }
            })?;
        if let Some(r) = t.refresh {
            l.refresh_token = r;
        }
        l.fornyet = chrono::Utc::now();
        if let Err(e) = l.gem(fil) {
            tracing::warn!("kunne ikke gemme fornyet login: {e:#}");
        }
        Ok((t.access, t.udloeber_sek))
    }

    /// Holder et brugerlogin i live: Microsoft lader et refresh-token udløbe efter 90 dage uden brug.
    /// Kaldes jævnligt; fornyer højst én gang i døgnet.
    pub async fn hold_i_live(&self) {
        let crate::config::Noegle::Login(fil) = &self.adgang.noegle else { return };
        let Ok(l) = crate::oauth::Login::laes(fil) else { return };
        if chrono::Utc::now() - l.fornyet < chrono::Duration::hours(20) {
            return;
        }
        self.glem_token().await;
        match self.token().await {
            Ok(_) => tracing::info!("login for {} er fornyet", l.konto),
            Err(e) => tracing::warn!("kunne ikke forny login: {e}"),
        }
    }

    fn bruger_url(&self) -> String {
        format!("{}/users/{}", self.graph_url, self.adgang.postkasse)
    }

    /// Sender en mail fra køen.
    pub async fn send(&self, raw: &[u8], env_fra: &str, til: &[String], navn: Option<&str>, svar: bool) -> Result<String, Fejl> {
        let afs = Afsender { from: &self.adgang.from, navn, svar_til_original: svar };
        let r = if raw.len() <= MIME_GRAENSE {
            self.send_mime(&mime::omskriv(raw, env_fra, til, &afs)).await.map(|_| "MIME".to_string())
        } else {
            self.send_stor(raw, env_fra, til, &afs).await
        };
        if let Err(f) = &r {
            if f.tekst.contains("401") {
                self.glem_token().await;
            }
        }
        r
    }

    async fn send_mime(&self, mime: &[u8]) -> Result<(), Fejl> {
        let tok = self.token().await?;
        let krop = base64::engine::general_purpose::STANDARD.encode(mime);
        let svar = self
            .http
            .post(format!("{}/sendMail", self.bruger_url()))
            .bearer_auth(tok)
            .header("Content-Type", "text/plain")
            .body(krop)
            .send()
            .await
            .map_err(|e| midl(format!("sendMail: {e}")))?;
        tjek("sendMail", svar).await.map(|_| ())
    }

    async fn send_stor(&self, raw: &[u8], env_fra: &str, til: &[String], afs: &Afsender<'_>) -> Result<String, Fejl> {
        let (besked, vedh) = mime::til_json(raw, env_fra, til, afs);
        let tok = self.token().await?;
        let svar = self
            .http
            .post(format!("{}/messages", self.bruger_url()))
            .bearer_auth(&tok)
            .json(&besked)
            .send()
            .await
            .map_err(|e| midl(format!("opret kladde: {e}")))?;
        let v = tjek("opret kladde", svar).await?;
        let id = v["id"].as_str().ok_or_else(|| midl("opret kladde: intet id"))?.to_string();
        let r = self.vedhaeft_og_send(&tok, &id, &vedh).await;
        if r.is_err() {
            let _ = self
                .http
                .delete(format!("{}/messages/{id}", self.bruger_url()))
                .bearer_auth(&tok)
                .send()
                .await;
        }
        r.map(|_| format!("kladde, {} vedhæftning(er)", vedh.len()))
    }

    async fn vedhaeft_og_send(&self, tok: &str, id: &str, vedh: &[mime::Vedhaeftning]) -> Result<(), Fejl> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let base = format!("{}/messages/{id}", self.bruger_url());
        for v in vedh {
            if v.data.len() <= LILLE_VEDH {
                let mut a = json!({
                    "@odata.type": "#microsoft.graph.fileAttachment",
                    "name": v.navn,
                    "contentType": v.type_,
                    "contentBytes": b64.encode(&v.data),
                    "isInline": v.inline,
                });
                if let Some(c) = &v.content_id {
                    a["contentId"] = json!(c);
                }
                let s = self
                    .http
                    .post(format!("{base}/attachments"))
                    .bearer_auth(tok)
                    .json(&a)
                    .send()
                    .await
                    .map_err(|e| midl(format!("vedhæft {}: {e}", v.navn)))?;
                tjek("vedhæft", s).await?;
                continue;
            }
            let mut item = json!({
                "attachmentType": "file",
                "name": v.navn,
                "size": v.data.len(),
                "contentType": v.type_,
                "isInline": v.inline,
            });
            if let Some(c) = &v.content_id {
                item["contentId"] = json!(c);
            }
            let s = self
                .http
                .post(format!("{base}/attachments/createUploadSession"))
                .bearer_auth(tok)
                .json(&json!({ "AttachmentItem": item }))
                .send()
                .await
                .map_err(|e| midl(format!("upload-session: {e}")))?;
            let sv = tjek("upload-session", s).await?;
            let url = sv["uploadUrl"].as_str().ok_or_else(|| midl("upload-session: ingen uploadUrl"))?.to_string();
            let i_alt = v.data.len();
            let mut fra = 0;
            while fra < i_alt {
                let til = (fra + BID).min(i_alt);
                // Upload-URL'en har sin egen nøgle; Authorization må ikke sendes med.
                let s = self
                    .http
                    .put(&url)
                    .header("Content-Type", "application/octet-stream")
                    .header("Content-Range", format!("bytes {fra}-{}/{i_alt}", til - 1))
                    .body(v.data[fra..til].to_vec())
                    .send()
                    .await
                    .map_err(|e| midl(format!("upload {}: {e}", v.navn)))?;
                tjek("upload", s).await?;
                fra = til;
            }
        }
        let s = self
            .http
            .post(format!("{base}/send"))
            .bearer_auth(tok)
            .header("Content-Length", "0")
            .send()
            .await
            .map_err(|e| midl(format!("send kladde: {e}")))?;
        tjek("send kladde", s).await.map(|_| ())
    }
}

async fn tjek(hvad: &str, svar: reqwest::Response) -> Result<Value, Fejl> {
    let status = svar.status();
    let tekst = svar.text().await.unwrap_or_default();
    if status.is_success() {
        return Ok(serde_json::from_str(&tekst).unwrap_or(Value::Null));
    }
    let v: Value = serde_json::from_str(&tekst).unwrap_or(Value::Null);
    let kode = v["error"]["code"].as_str().unwrap_or("");
    let besked = v["error"]["message"].as_str().unwrap_or(tekst.as_str());
    let besked: String = besked.chars().take(300).collect();
    let n = status.as_u16();
    // 400/413 bliver ikke bedre. 403/404 kan skyldes, at rettigheder ikke er slået igennem endnu
    // (op til 2 timer), så dem prøver vi igen.
    let midlertidig = !matches!(n, 400 | 413 | 415);
    Err(Fejl { midlertidig, tekst: format!("{hvad}: {n} {kode} {besked}").trim().to_string() })
}
