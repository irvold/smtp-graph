//! SMTP-Graph: tager imod SMTP fra scannere o.l. og sender videre gennem Microsoft Graph.
//! Biblioteket bruges af både tjenesten/kommandolinjen (smtp-graph.exe) og GUI'en.

pub mod config;
pub mod graph;
pub mod koe;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod mime;
pub mod oauth;
pub mod opsaet;
pub mod reserve;
#[cfg(windows)]
pub mod service;
pub mod skriv;
pub mod smtp;
pub mod tls;

use anyhow::Result;
use chrono::Utc;
use config::Config;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

pub fn std_config() -> PathBuf {
    let ved_siden = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("smtp-graph.toml")))
        .unwrap_or_else(|| "smtp-graph.toml".into());
    // Linux: /var/lib/smtp-graph/smtp-graph.toml, medmindre der ligger en fil ved siden af programmet.
    #[cfg(target_os = "linux")]
    if !ved_siden.exists() {
        return linux::std_config();
    }
    ved_siden
}


pub fn testmail(til: &str, stor: bool) -> Vec<u8> {
    let dato = Utc::now().to_rfc2822();
    let mut m = format!(
        "To: <{til}>\r\nSubject: Test fra SMTP-Graph\r\nDate: {dato}\r\nMIME-Version: 1.0\r\n"
    );
    let tekst = format!("Dette er en test fra SMTP-Graph {}.\r\nSendt {dato}.\r\n", env!("CARGO_PKG_VERSION"));
    if !stor {
        m += "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n";
        m += &tekst;
        return m.into_bytes();
    }
    use base64::Engine;
    let data: Vec<u8> = (0..5_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
    m += "Content-Type: multipart/mixed; boundary=\"GRAENSE\"\r\n\r\n--GRAENSE\r\n";
    m += "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n";
    m += &tekst;
    m += "\r\n--GRAENSE\r\nContent-Type: application/octet-stream; name=\"test-5mb.bin\"\r\n";
    m += "Content-Disposition: attachment; filename=\"test-5mb.bin\"\r\nContent-Transfer-Encoding: base64\r\n\r\n";
    for bid in b64.as_bytes().chunks(76) {
        m += std::str::from_utf8(bid).unwrap();
        m += "\r\n";
    }
    m += "--GRAENSE--\r\n";
    m.into_bytes()
}

pub fn log(cfg: &Config, konsol: bool) -> tracing_appender::non_blocking::WorkerGuard {
    use tracing_subscriber::prelude::*;
    let mappe = cfg.mappe.join("log");
    let _ = std::fs::create_dir_all(&mappe);
    let fil = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("smtp-graph")
        .filename_suffix("log")
        .max_log_files(60)
        .build(&mappe)
        .expect("logmappe");
    let (nb, guard) = tracing_appender::non_blocking(fil);
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,reqwest=warn,rustls=warn".into())
    };
    let fil_lag = tracing_subscriber::fmt::layer().with_ansi(false).with_writer(nb).with_filter(filter());
    let farver = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let konsol_lag = konsol.then(|| tracing_subscriber::fmt::layer().with_ansi(farver).with_filter(filter()));
    let _ = tracing_subscriber::registry().with(fil_lag).with(konsol_lag).try_init();
    guard
}

/// Selve tjenesten: lyttere + afsender, indtil `stop` bliver sand.
pub async fn koer(mut cfg: Config, mut stop: watch::Receiver<bool>) -> Result<()> {
    tracing::info!("SMTP-Graph {} starter med {}", env!("CARGO_PKG_VERSION"), cfg.fil.display());
    let graph = if cfg.graph.toerlob {
        tracing::warn!("TØRLØB: intet sendes; mails gemmes i {}", cfg.koe.mappe.join("toerlob").display());
        None
    } else {
        let a = cfg.graph_adgang()?;
        tracing::info!("sender fra {} (postkasse {}, {})", a.from, a.postkasse, a.konto_tekst());
        if let Some(u) = cfg.graph.udloeber.as_deref() {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(u, "%Y-%m-%d") {
                let dage = (d - Utc::now().date_naive()).num_days();
                if dage < 30 {
                    tracing::warn!("app-hemmeligheden udløber {u} (om {dage} dage) – kør Opsaet-SmtpGraph.ps1 -Forny");
                }
            }
        }
        Some(graph::Graph::ny(a, cfg.graph.login_url.clone(), cfg.graph.graph_url.clone()))
    };
    let tls = tls::Tls::ny(&cfg)?;
    if let Some(t) = &tls {
        tracing::info!("TLS: {}", t.motor_tekst());
    }
    let koe = koe::Koe::ny(&cfg.koe.mappe)?;
    let n = koe.antal().await;
    if n > 0 {
        tracing::info!("{n} mail(s) venter i køen");
    }
    let f = Arc::new(smtp::Faelles { cfg: cfg.clone(), koe, tls, ny_post: tokio::sync::Notify::new() });

    let mut opgaver = tokio::task::JoinSet::new();
    for adr in f.cfg.smtp.lyt.clone() {
        opgaver.spawn(smtp::lyt(f.clone(), adr, false));
    }
    for adr in f.cfg.smtp.lyt_tls.clone() {
        opgaver.spawn(smtp::lyt(f.clone(), adr, true));
    }
    let reserve = if f.cfg.reserve.aktiv() && graph.is_some() {
        let r = reserve::Reserve::ny(&f.cfg.reserve, &f.cfg.smtp.vaertsnavn)?;
        tracing::info!("reserve: {} (efter {} min)", f.cfg.reserve.beskrivelse(), f.cfg.reserve.efter_minutter);
        Some(r)
    } else {
        None
    };
    let afsender = tokio::spawn(afsend(f.clone(), graph, reserve, stop.clone()));

    tokio::select! {
        r = opgaver.join_next() => {
            if let Some(Ok(Err(e))) = r {
                tracing::error!("{e:#}");
                return Err(e);
            }
        }
        _ = stop.wait_for(|s| *s) => {}
    }
    opgaver.abort_all();
    let _ = tokio::time::timeout(Duration::from_secs(20), afsender).await;
    tracing::info!("stoppet");
    Ok(())
}

async fn afsend(f: Arc<smtp::Faelles>, graph: Option<graph::Graph>, reserve: Option<reserve::Reserve>, mut stop: watch::Receiver<bool>) {
    let cfg = &f.cfg;
    // Hvornår fejl_til sidst fik besked om, at reserven er i brug (højst hver 6. time).
    let mut sidst_varslet: Option<std::time::Instant> = None;
    loop {
        let klar = match f.koe.klar().await {
            Ok(k) => k,
            Err(e) => {
                tracing::error!("kan ikke læse køen: {e:#}");
                vec![]
            }
        };
        for mut post in klar {
            if *stop.borrow() {
                return;
            }
            let raw = match tokio::fs::read(f.koe.eml(&post.id)).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("{}: kan ikke læse mailen: {e}", post.id);
                    f.koe.faerdig(&post.id).await;
                    continue;
                }
            };
            let Some(g) = graph.as_ref() else {
                let _ = f.koe.flyt(&post, "toerlob").await;
                tracing::info!("{}: TØRLØB – ville sende til {}", post.id, post.til.join(", "));
                continue;
            };
            post.forsoeg += 1;
            let navn = cfg.graph.afsender_navn.as_deref();
            let e = match g.send(&raw, &post.fra, &post.til, navn, cfg.graph.svar_til_original).await {
                Ok(vej) => {
                    tracing::info!("{}: sendt til {} ({vej}, forsøg {})", post.id, post.til.join(", "), post.forsoeg);
                    f.koe.faerdig(&post.id).await;
                    continue;
                }
                Err(e) => e,
            };
            post.sidste_fejl = Some(e.tekst.clone());
            let mut midlertidig = e.midlertidig;

            // Reserven: med det samme, hvis Microsoft afviser helt, ellers når mailen har ventet længe nok.
            if let Some(r) = reserve.as_ref() {
                let ventet = Utc::now() - post.modtaget;
                if !e.midlertidig || ventet >= chrono::Duration::minutes(r.cfg().efter_minutter as i64) {
                    match r.send(&raw, &post.fra, &post.til, navn, cfg.graph.svar_til_original).await {
                        Ok(vej) => {
                            tracing::warn!(
                                "{}: Microsoft fejlede ({e}) – sendt til {} via {vej} (forsøg {})",
                                post.id,
                                post.til.join(", "),
                                post.forsoeg
                            );
                            f.koe.faerdig(&post.id).await;
                            if let Some(adm) = cfg.graph.fejl_til.as_deref() {
                                if sidst_varslet.is_none_or(|t| t.elapsed() > Duration::from_secs(6 * 3600)) {
                                    sidst_varslet = Some(std::time::Instant::now());
                                    let besked = reservebesked(adm, &e.tekst, &r.cfg().beskrivelse());
                                    if let Err(e2) = r.send(&besked, "", &[adm.to_string()], navn, false).await {
                                        tracing::error!("kunne ikke give {adm} besked om reserven: {e2}");
                                    }
                                }
                            }
                            continue;
                        }
                        Err(e2) => {
                            tracing::warn!("{}: reserven fejlede også: {e2}", post.id);
                            post.sidste_fejl = Some(format!("{}; {}", e.tekst, e2.tekst));
                            midlertidig = e.midlertidig || e2.midlertidig;
                        }
                    }
                }
            }

            let gammel = Utc::now() - post.modtaget > chrono::Duration::hours(cfg.koe.max_timer as i64);
            if midlertidig && !gammel {
                post.naeste = Utc::now() + koe::Koe::pause(post.forsoeg);
                tracing::warn!(
                    "{}: forsøg {} fejlede, prøver igen {}: {}",
                    post.id,
                    post.forsoeg,
                    post.naeste.format("%H:%M"),
                    post.sidste_fejl.as_deref().unwrap_or("")
                );
                let _ = f.koe.gem(&post).await;
            } else {
                tracing::error!("{}: opgivet efter {} forsøg: {}", post.id, post.forsoeg, post.sidste_fejl.as_deref().unwrap_or(""));
                let _ = f.koe.flyt(&post, "fejlet").await;
                if let Some(adm) = cfg.graph.fejl_til.as_deref() {
                    let besked = fejlbesked(adm, &post);
                    if let Err(e2) = g.send(&besked, "", &[adm.to_string()], navn, false).await {
                        match reserve.as_ref() {
                            Some(r) => {
                                if let Err(e3) = r.send(&besked, "", &[adm.to_string()], navn, false).await {
                                    tracing::error!("kunne heller ikke give besked til {adm}: {e2}; {e3}");
                                }
                            }
                            None => tracing::error!("kunne heller ikke give besked til {adm}: {e2}"),
                        }
                    }
                }
            }
        }
        if let Some(g) = graph.as_ref() {
            g.hold_i_live().await;
        }
        tokio::select! {
            _ = f.ny_post.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(20)) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
    }
}

fn reservebesked(til: &str, fejl: &str, reserve: &str) -> Vec<u8> {
    format!(
        "To: <{til}>\r\nSubject: SMTP-Graph sender via reserven\r\nDate: {}\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n\
         Microsoft kunne ikke sende, så mails fra scanneren går nu gennem reserven ({reserve}).\r\n\r\n\
         Fejlen fra Microsoft: {fejl}\r\n\r\n\
         Modtagerne ser reserve-adressen som afsender. Du får højst én besked hver 6. time.\r\n",
        Utc::now().to_rfc2822()
    )
    .into_bytes()
}

fn fejlbesked(til: &str, p: &koe::Post) -> Vec<u8> {
    format!(
        "To: <{til}>\r\nSubject: SMTP-Graph kunne ikke sende en mail\r\nDate: {}\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n\
         En mail kunne ikke sendes og er lagt i mappen koe\\fejlet.\r\n\r\n\
         Id: {}\r\nFra: {}\r\nTil: {}\r\nAfsendt af: {}\r\nModtaget: {}\r\nForsøg: {}\r\nFejl: {}\r\n",
        Utc::now().to_rfc2822(),
        p.id,
        p.fra,
        p.til.join(", "),
        p.klient,
        p.modtaget.with_timezone(&chrono::Local).format("%d-%m-%Y %H:%M"),
        p.forsoeg,
        p.sidste_fejl.as_deref().unwrap_or("")
    )
    .into_bytes()
}
