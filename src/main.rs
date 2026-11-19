//! Kommandolinje og Windows-tjeneste til SMTP-Graph.

use anyhow::Result;
use clap::{Parser, Subcommand};
use smtp_graph::config::{self, Config};
use smtp_graph::{graph, koe, koer, log, std_config, testmail};
use std::path::PathBuf;
use tokio::sync::watch;

#[derive(Parser)]
#[command(name = "smtp-graph", version, about = "SMTP ind, Microsoft Graph ud")]
struct Cli {
    /// Indstillingsfil (standard: smtp-graph.toml ved siden af programmet)
    #[arg(long, short, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    kmd: Option<Kmd>,
}

#[derive(Subcommand)]
enum Kmd {
    /// Kør i konsollen (standard)
    Koer,
    /// Skriv en ny indstillingsfil med forklaringer
    NyConfig,
    /// Tjek indstillinger og login hos Microsoft
    Tjek,
    /// Send en testmail direkte gennem Graph
    Test {
        #[arg(long)]
        til: String,
        /// Med en vedhæftning på ca. 5 MB (afprøver upload-vejen, kræver Mail.ReadWrite)
        #[arg(long)]
        stor: bool,
    },
    /// Vis køen
    Koe,
    /// Sæt Microsoft 365 op: log ind som administrator hos kunden, og programmet opretter kundens app
    Opsaet {
        #[arg(long)]
        postkasse: String,
        /// Stedets navn (bruges i appens navn hos kunden)
        #[arg(long)]
        navn: String,
        #[arg(long)]
        send_som: Option<String>,
        /// Opret postkassen som delt postkasse, hvis den ikke findes
        #[arg(long)]
        opret_delt: bool,
        /// Kun Mail.Send (ingen mails over ca. 3 MB)
        #[arg(long)]
        kun_send: bool,
    },
    /// Log ind som den konto, der skal sende (metoden "bruger")
    Login {
        /// Send fra en anden (delt) postkasse end kontoen
        #[arg(long)]
        send_fra: Option<String>,
    },
    /// Installér som tjeneste (Windows-tjeneste eller systemd; kræver administrator/root)
    Installer,
    /// Fjern tjenesten
    Afinstaller,
    /// Bruges af tjenestestyringen (Windows eller systemd)
    #[command(hide = true)]
    Tjeneste,
}

fn main() -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();
    let fil = cli.config.clone().unwrap_or_else(std_config);
    let kmd = cli.kmd.unwrap_or(Kmd::Koer);
    // Linux: `sudo smtp-graph tjek` o.l. kører som tjenestens bruger, så filerne får den rigtige ejer.
    #[cfg(target_os = "linux")]
    if !matches!(kmd, Kmd::Installer | Kmd::Afinstaller) {
        smtp_graph::linux::som_ejer(&fil)?;
        // Uden skærm kan login-adressen indsættes i terminalen.
        smtp_graph::oauth::tillad_indsaet();
    }
    match kmd {
        Kmd::NyConfig => {
            if fil.exists() {
                anyhow::bail!("{} findes allerede", fil.display());
            }
            std::fs::write(&fil, config::SKABELON)?;
            println!("Skrev {}", fil.display());
            Ok(())
        }
        Kmd::Koer => koer_her(&fil),
        Kmd::Tjeneste => {
            #[cfg(windows)]
            return smtp_graph::service::dispatch(fil);
            #[cfg(not(windows))]
            return koer_her(&fil);
        }
        Kmd::Installer => {
            #[cfg(windows)]
            let linjer = smtp_graph::service::installer(&fil, &std::env::current_exe()?)?;
            #[cfg(target_os = "linux")]
            let linjer = smtp_graph::linux::installer(&cli.config.clone().unwrap_or_else(smtp_graph::linux::std_config))?;
            #[cfg(not(any(windows, target_os = "linux")))]
            let linjer: Vec<String> = anyhow::bail!("kun Windows og Linux (systemd)");
            for l in linjer {
                println!("{l}");
            }
            Ok(())
        }
        Kmd::Afinstaller => {
            #[cfg(windows)]
            println!("{}", smtp_graph::service::afinstaller()?);
            #[cfg(target_os = "linux")]
            println!("{}", smtp_graph::linux::afinstaller()?);
            #[cfg(not(any(windows, target_os = "linux")))]
            anyhow::bail!("kun Windows og Linux (systemd)");
            Ok(())
        }
        Kmd::Tjek => {
            let mut cfg = Config::laes(&fil)?;
            let a = cfg.graph_adgang()?;
            println!("Indstillinger: {}", cfg.fil.display());
            println!("Tenant:        {}", a.tenant);
            println!("App (klient):  {}", a.client_id);
            println!("Postkasse:     {}", a.postkasse);
            println!("Afsender:      {}", a.from);
            if let Some(u) = &cfg.graph.udloeber {
                println!("Hemmelighed udløber: {u}");
            }
            println!("Lytter:        {:?} {:?}", cfg.smtp.lyt, cfg.smtp.lyt_tls);
            println!("Tilladte IP:   {:?}", cfg.smtp.tilladte_ip);
            let rt = tokio::runtime::Runtime::new()?;
            let g = graph::Graph::ny(a, cfg.graph.login_url.clone(), cfg.graph.graph_url.clone());
            match rt.block_on(g.token()) {
                Ok(_) => println!("Login hos Microsoft: OK"),
                Err(e) => println!("Login hos Microsoft: FEJL – {e}"),
            }
            Ok(())
        }
        Kmd::Test { til, stor } => {
            let mut cfg = Config::laes(&fil)?;
            let a = cfg.graph_adgang()?;
            let g = graph::Graph::ny(a, cfg.graph.login_url.clone(), cfg.graph.graph_url.clone());
            let raw = testmail(&til, stor);
            let rt = tokio::runtime::Runtime::new()?;
            let r = rt.block_on(g.send(&raw, "", &[til.clone()], cfg.graph.afsender_navn.as_deref(), false));
            match r {
                Ok(vej) => println!("Sendt til {til} ({vej}, {} kB)", raw.len() / 1024),
                Err(e) => anyhow::bail!("{e}"),
            }
            Ok(())
        }
        Kmd::Opsaet { postkasse, navn, send_som, opret_delt, kun_send } => {
            let cfg = Config::laes(&fil)?;
            let valg = smtp_graph::opsaet::Valg { postkasse, navn, send_som, opret_delt, kun_send, maaneder: 24 };
            let log = |t: String| println!("{t}");
            let rt = tokio::runtime::Runtime::new()?;
            let r = rt.block_on(smtp_graph::opsaet::admin(&cfg.faelles_app(), &valg, &log))?;
            smtp_graph::skriv::app_kode(&cfg.fil, &r.kode)?;
            println!("Gemt i {}: app {} sender fra {} (hemmelighed udløber {}).", cfg.fil.display(), r.app_id, r.postkasse, r.udloeber);
            Ok(())
        }
        Kmd::Login { send_fra } => {
            let cfg = Config::laes(&fil)?;
            let log = |t: String| println!("{t}");
            let rt = tokio::runtime::Runtime::new()?;
            let (login, mail) = rt.block_on(smtp_graph::opsaet::bruger(&cfg.faelles_app(), &log))?;
            login.gem(&cfg.login_fil())?;
            let send_fra = send_fra.unwrap_or_default();
            smtp_graph::skriv::bruger(&cfg.fil, &send_fra)?;
            println!("Gemt: sender som {} (logget ind som {}).", if send_fra.is_empty() { &mail } else { &send_fra }, login.konto);
            Ok(())
        }
        Kmd::Koe => {
            let cfg = Config::laes(&fil)?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                let k = koe::Koe::ny(&cfg.koe.mappe)?;
                let mut alle = Vec::new();
                for under in ["", "fejlet"] {
                    let d = k.mappe.join(under);
                    for e in std::fs::read_dir(&d)?.flatten() {
                        let p = e.path();
                        if p.extension().and_then(|x| x.to_str()) == Some("json") {
                            if let Ok(post) = serde_json::from_slice::<koe::Post>(&std::fs::read(&p)?) {
                                alle.push((if under.is_empty() { "venter" } else { "fejlet" }, post));
                            }
                        }
                    }
                }
                if alle.is_empty() {
                    println!("Køen er tom ({}).", k.mappe.display());
                }
                for (st, p) in alle {
                    println!(
                        "{st:7} {} fra {} til {} – {} forsøg{}",
                        p.id,
                        p.fra,
                        p.til.join(", "),
                        p.forsoeg,
                        p.sidste_fejl.map(|f| format!(" – {f}")).unwrap_or_default()
                    );
                }
                Ok(())
            })
        }
    }
}

/// Kør i forgrunden, til Ctrl+C (eller SIGTERM fra systemd).
fn koer_her(fil: &std::path::Path) -> Result<()> {
    let cfg = Config::laes(fil)?;
    let _g = log(&cfg, true);
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let (tx, rx) = watch::channel(false);
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => tracing::info!("stopper (Ctrl+C)"),
                    _ = term.recv() => tracing::info!("stopper (SIGTERM)"),
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("stopper (Ctrl+C)");
            }
            let _ = tx.send(true);
        });
        koer(cfg, rx).await
    })
}
