//! Administrator-opsætning uden PowerShell: én browser-login som administrator hos kunden,
//! hvorefter programmet selv opretter kundens app, hemmelighed og Exchange-afgrænsning
//! (samme resultat som Opsaet-SmtpGraph.ps1).
//!
//! Exchange-delen bruger den REST-indgang, som Microsofts eget ExchangeOnlineManagement-modul
//! (v3) bruger til alle cmdlets: POST outlook.office365.com/adminapi/beta/{tenant}/InvokeCommand.

use crate::oauth;
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::time::Duration;

pub struct Valg {
    pub postkasse: String,
    pub navn: String,
    pub send_som: Option<String>,
    pub opret_delt: bool,
    pub kun_send: bool,
    pub maaneder: u32,
}

pub struct Resultat {
    pub kode: String,
    pub postkasse: String,
    pub app_id: String,
    pub udloeber: String,
}

type Log<'a> = &'a (dyn Fn(String) + Sync);

struct Klient {
    http: reqwest::Client,
    graph: String,
    exo: String,
    tenant: String,
    anker: String,
}

impl Klient {
    async fn graph(&self, metode: reqwest::Method, sti: &str, krop: Option<Value>) -> Result<Value> {
        let mut r = self.http.request(metode, format!("https://graph.microsoft.com/v1.0{sti}")).bearer_auth(&self.graph);
        if let Some(k) = krop {
            r = r.json(&k);
        }
        let s = r.send().await?;
        let st = s.status();
        let t = s.text().await.unwrap_or_default();
        if !st.is_success() {
            let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
            bail!("Graph {sti}: {st} {}", v["error"]["message"].as_str().unwrap_or(&t));
        }
        Ok(serde_json::from_str(&t).unwrap_or(Value::Null))
    }

    /// Kør en Exchange-cmdlet. "Findes ikke"-fejl giver en tom liste.
    async fn exo(&self, cmdlet: &str, param: Value) -> Result<Vec<Value>> {
        let krop = json!({ "CmdletInput": { "CmdletName": cmdlet, "Parameters": param } });
        let s = self
            .http
            .post(format!("https://outlook.office365.com/adminapi/beta/{}/InvokeCommand", self.tenant))
            .bearer_auth(&self.exo)
            .header("X-AnchorMailbox", &self.anker)
            .header("X-ResponseFormat", "json")
            .json(&krop)
            .send()
            .await?;
        let st = s.status();
        let t = s.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
        if std::env::var_os("SMTPGRAPH_DEBUG").is_some() {
            eprintln!("EXO {cmdlet} {} → {st}\n{}", krop, t.chars().take(3000).collect::<String>());
        }
        if !st.is_success() {
            // Selve forklaringen ligger ofte i error.details[].message eller error.innererror.
            let mut dele = vec![v["error"]["message"].as_str().unwrap_or("").to_string()];
            if let Some(d) = v["error"]["details"].as_array() {
                dele.extend(d.iter().filter_map(|x| x["message"].as_str().map(str::to_string)));
            }
            if let Some(i) = v["error"]["innererror"].as_object() {
                dele.extend(i.values().filter_map(|x| x.as_str().map(str::to_string)));
            }
            let mut besked = dele.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" | ");
            if besked.is_empty() {
                besked = t.clone();
            }
            if besked.contains("couldn't be found") || besked.contains("ManagementObjectNotFound") || besked.contains("kunne ikke findes") {
                return Ok(vec![]);
            }
            bail!("{cmdlet}: {st} {}", besked.chars().take(1500).collect::<String>());
        }
        Ok(v["value"].as_array().cloned().unwrap_or_default())
    }
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

pub async fn admin(faelles_app: &str, valg: &Valg, log: Log<'_>) -> Result<Resultat> {
    let postkasse = valg.postkasse.trim().to_lowercase();
    if !postkasse.contains('@') {
        bail!("»{postkasse}« er ikke en mailadresse");
    }
    let app_navn = format!("SMTP-Graph – {}", valg.navn.trim());

    // 1. Login som administrator (Graph + Exchange godkendes i samme omgang).
    let t = oauth::browser_login(faelles_app, oauth::SCOPE_ADMIN_GRAPH, "", log).await?;
    log(format!("Logget ind som {} (tenant {})", t.konto, t.tenant));
    let refresh = t.refresh.clone().context("Microsoft gav intet refresh-token")?;
    let exo = oauth::forny(faelles_app, &t.tenant, &refresh, &format!("{} offline_access", oauth::SCOPE_ADMIN_EXO), None)
        .await
        .context("adgang til Exchange Online (kontoen skal være Exchange- eller global administrator)")?;
    let k = Klient {
        http: reqwest::Client::builder().timeout(Duration::from_secs(120)).build()?,
        graph: t.access.clone(),
        exo: exo.access,
        tenant: t.tenant.clone(),
        anker: format!("UPN:{}", t.konto),
    };

    // 2. Postkassen.
    log(format!("Finder postkassen {postkasse} …"));
    let mut mbx = k.exo("Get-Mailbox", json!({ "Identity": postkasse })).await?;
    if mbx.is_empty() && valg.opret_delt {
        log(format!("Opretter den delte postkasse {postkasse} …"));
        let navn = format!("Scanner {}", valg.navn.trim());
        k.exo("New-Mailbox", json!({ "Shared": true, "Name": navn, "DisplayName": navn, "PrimarySmtpAddress": postkasse })).await?;
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_secs(5)).await;
            mbx = k.exo("Get-Mailbox", json!({ "Identity": postkasse })).await?;
            if !mbx.is_empty() {
                break;
            }
        }
    }
    let Some(m) = mbx.first() else {
        bail!("Postkassen {postkasse} findes ikke hos kunden. Opret den (gerne som delt postkasse), eller sæt flueben i »Opret postkassen«.");
    };
    let type_ = s(m, "RecipientTypeDetails");
    let primaer = s(m, "PrimarySmtpAddress").to_lowercase();
    let postkasse = if primaer.is_empty() { postkasse } else { primaer };
    log(format!("Postkasse: {} ({type_})", s(m, "DisplayName")));
    if type_ == "UserMailbox" {
        log("Bemærk: en personlig postkasse. En delt postkasse kun til scanneren er sikrere.".into());
    }
    let scope_navn = format!("SMTP-Graph – {postkasse}");

    // 3. Kundens egen app i Entra.
    let filter = app_navn.replace('\'', "''");
    let fundne = k
        .graph(reqwest::Method::GET, &format!("/applications?$filter=displayName eq '{}'&$select=id,appId,displayName", urlenc(&filter)), None)
        .await?;
    let app = match fundne["value"].as_array().and_then(|a| a.first()).cloned() {
        Some(a) => {
            log(format!("Appen »{app_navn}« findes allerede – den genbruges."));
            a
        }
        None => {
            log(format!("Opretter appen »{app_navn}« …"));
            k.graph(
                reqwest::Method::POST,
                "/applications",
                Some(json!({
                    "displayName": app_navn,
                    "signInAudience": "AzureADMyOrg",
                    "notes": format!("Bruges af SMTP-Graph ({}) til at sende scannermail fra {postkasse}. Adgangen er afgrænset i Exchange Online (RBAC for Applications).", valg.navn.trim()),
                })),
            )
            .await?
        }
    };
    let app_id = s(&app, "appId");
    let app_obj = s(&app, "id");
    let sp = k.graph(reqwest::Method::GET, &format!("/servicePrincipals?$filter=appId eq '{app_id}'&$select=id"), None).await?;
    let sp_id = match sp["value"].as_array().and_then(|a| a.first()) {
        Some(x) => s(x, "id"),
        None => s(&k.graph(reqwest::Method::POST, "/servicePrincipals", Some(json!({ "appId": app_id }))).await?, "id"),
    };

    log("Laver en klienthemmelighed …".into());
    let udl = chrono::Utc::now() + chrono::Months::new(valg.maaneder.clamp(1, 24));
    let pw = k
        .graph(
            reqwest::Method::POST,
            &format!("/applications/{app_obj}/addPassword"),
            Some(json!({ "passwordCredential": {
                "displayName": format!("SMTP-Graph {} {}", valg.navn.trim(), chrono::Local::now().format("%Y-%m-%d")),
                "endDateTime": udl.to_rfc3339(),
            }})),
        )
        .await?;
    let secret = s(&pw, "secretText");
    if secret.is_empty() {
        bail!("Microsoft returnerede ingen hemmelighed");
    }

    // 4. Exchange: app'en må kun bruge postkassen.
    log("Registrerer appen i Exchange …".into());
    if k.exo("Get-ServicePrincipal", json!({ "Identity": sp_id })).await?.is_empty() {
        let mut ok = false;
        for forsoeg in 1..=30 {
            match k.exo("New-ServicePrincipal", json!({ "AppId": app_id, "ObjectId": sp_id, "DisplayName": app_navn })).await {
                Ok(_) => {
                    ok = true;
                    break;
                }
                Err(e) if forsoeg < 30 => {
                    log(format!("  venter på, at Exchange kan se appen ({forsoeg}/30) …"));
                    tracing::debug!("New-ServicePrincipal: {e:#}");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                Err(e) => return Err(e),
            }
        }
        if !ok {
            bail!("Exchange kunne ikke se den nye app");
        }
    }
    if k.exo("Get-ManagementScope", json!({ "Identity": scope_navn })).await?.is_empty() {
        k.exo("New-ManagementScope", json!({ "Name": scope_navn, "RecipientRestrictionFilter": format!("PrimarySmtpAddress -eq '{postkasse}'") })).await?;
        log(format!("Afgrænsning oprettet: kun {postkasse}"));
    }
    let mut roller = vec!["Application Mail.Send"];
    if !valg.kun_send {
        roller.push("Application Mail.ReadWrite");
    }
    for rolle in &roller {
        let fandtes = k
            .exo("Get-ManagementRoleAssignment", json!({ "RoleAssignee": sp_id, "Role": rolle }))
            .await?
            .iter()
            .any(|r| s(r, "CustomResourceScope") == scope_navn);
        if fandtes {
            log(format!("Rollen »{rolle}« fandtes allerede"));
        } else {
            k.exo("New-ManagementRoleAssignment", json!({ "App": sp_id, "Role": rolle, "CustomResourceScope": scope_navn })).await?;
            log(format!("Rollen »{rolle}« er tildelt"));
        }
    }
    let from = match valg.send_som.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
        Some(ss) if !ss.eq_ignore_ascii_case(&postkasse) => {
            log(format!("Giver {postkasse} lov til at sende som {ss} …"));
            let har = k
                .exo("Get-RecipientPermission", json!({ "Identity": ss, "Trustee": postkasse }))
                .await?
                .iter()
                .any(|p| p["AccessRights"].to_string().contains("SendAs"));
            if !har {
                k.exo("Add-RecipientPermission", json!({ "Identity": ss, "Trustee": postkasse, "AccessRights": ["SendAs"], "Confirm": false })).await?;
            }
            ss.to_lowercase()
        }
        _ => postkasse.clone(),
    };
    match k.exo("Test-ServicePrincipalAuthorization", json!({ "Identity": sp_id, "Resource": postkasse })).await {
        Ok(v) => {
            for rolle in &roller {
                let ok = v.iter().any(|r| s(r, "RoleName") == *rolle && (r["InScope"].as_bool() == Some(true) || r["InScope"].as_str().is_some_and(|x| x.eq_ignore_ascii_case("true"))));
                log(format!("  {}: {rolle}", if ok { "OK" } else { "venter stadig" }));
            }
        }
        Err(e) => log(format!("  (kunne ikke tjekke adgangen: {e:#})")),
    }

    let kode = format!(
        "SMTPGRAPH1:{}",
        base64::engine::general_purpose::STANDARD.encode(
            serde_json::to_vec(&json!({
                "tenant": t.tenant,
                "client_id": app_id,
                "secret": secret,
                "postkasse": postkasse,
                "from": from,
                "udloeber": udl.format("%Y-%m-%d").to_string(),
            }))?
        )
    );
    Ok(Resultat { kode, postkasse, app_id, udloeber: udl.format("%Y-%m-%d").to_string() })
}

/// Metoden "bruger": log ind som postkassen (eller en bruger med adgang til den).
pub async fn bruger(faelles_app: &str, log: Log<'_>) -> Result<(oauth::Login, String)> {
    let t = oauth::browser_login(faelles_app, oauth::SCOPE_BRUGER, "", log).await?;
    let refresh = t.refresh.clone().context("Microsoft gav intet refresh-token")?;
    // Brugerens egen mailadresse (kan afvige fra loginnavnet).
    let mig: Value = reqwest::Client::new()
        .get("https://graph.microsoft.com/v1.0/me?$select=mail,userPrincipalName")
        .bearer_auth(&t.access)
        .send()
        .await?
        .json()
        .await
        .unwrap_or(Value::Null);
    let mail = mig["mail"].as_str().filter(|m| !m.is_empty()).unwrap_or(&t.konto).to_lowercase();
    log(format!("Logget ind som {} ({mail})", t.konto));
    Ok((
        oauth::Login { klient: faelles_app.to_string(), tenant: t.tenant, konto: t.konto, refresh_token: refresh, fornyet: chrono::Utc::now() },
        mail,
    ))
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
