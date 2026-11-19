//! SMTP-Graph – opsætning, kø, log og styring af tjenesten.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod gem;

use eframe::egui::{self, Color32, RichText};
use smtp_graph::config::{Config, SKABELON};
use smtp_graph::koe::{self, Post, Status};
use smtp_graph::service::{self, ServiceState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SCRIPT: &str = include_str!("../../Opsaet-SmtpGraph.ps1");
const GROEN: Color32 = Color32::from_rgb(40, 160, 80);
const ROED: Color32 = Color32::from_rgb(210, 60, 50);
const GUL: Color32 = Color32::from_rgb(200, 150, 20);

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let har = |k: &str| args.iter().any(|a| a == k);
    let efter = |k: &str| args.iter().skip_while(|a| *a != k).nth(1).cloned();
    let mut admin = er_admin();
    if !admin && !har("--ingen-admin") && genstart_som_admin(&args[1..]) {
        return Ok(());
    }
    if har("--ingen-admin") {
        admin = er_admin();
    }
    // Til skærmbilleder i vejledningen: vis GUI'en som administrator med eksempeltekster.
    let demo = har("--demo");
    if demo {
        admin = true;
    }
    let shot = efter("--skaermbillede").map(PathBuf::from);
    let fane = match efter("--fane").as_deref() {
        Some("ms") => Fane::Microsoft,
        Some("modtager") => Fane::Modtager,
        Some("koe") => Fane::Koe,
        Some("log") => Fane::Log,
        Some("om") => Fane::Om,
        _ => Fane::Oversigt,
    };
    let cfg_fil = efter("--config").map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SMTP-Graph")
            .with_inner_size([1120.0, 780.0])
            .with_min_inner_size([820.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SMTP-Graph",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.all_styles_mut(|s| {
                for f in s.text_styles.values_mut() {
                    f.size *= 1.06;
                }
                s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            });
            let mut app = App::ny(admin, cfg_fil);
            app.fane = fane;
            if demo {
                app.ms.postkasse = "scanner@firma.dk".into();
                app.ms.navn = "Firma ApS kontor".into();
                app.ms.opret_delt = true;
                app.ms.test_til = "dig@firma.dk".into();
                app.ips = vec!["192.168.1.20".into()];
            }
            app.shot = shot.map(|p| (p, Instant::now(), false));
            Ok(Box::new(app))
        }),
    )
}

#[cfg(windows)]
fn er_admin() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

#[cfg(windows)]
fn genstart_som_admin(args: &[String]) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let w = |s: &std::ffi::OsStr| s.encode_wide().chain([0]).collect::<Vec<u16>>();
    let Ok(exe) = std::env::current_exe() else { return false };
    let param = args.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" ");
    let r = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            w("runas".as_ref()).as_ptr(),
            w(exe.as_os_str()).as_ptr(),
            w(param.as_ref()).as_ptr(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    r as isize > 32
}

/// Åbn en adresse i standardbrowseren.
fn aabn_adresse(url: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[derive(PartialEq, Clone, Copy)]
enum Fane {
    Oversigt,
    Microsoft,
    Modtager,
    Koe,
    Log,
    Om,
}

/// Et job i baggrunden med løbende tekst.
struct Job {
    titel: String,
    linjer: Arc<Mutex<Vec<String>>>,
    slut: Arc<Mutex<Option<Result<(), String>>>>,
    genindlaes: bool,
}

/// SMTP-indstillinger som tekstfelter.
#[derive(Default, Clone)]
pub struct Formular {
    pub lyt: String,
    pub lyt_tls: String,
    pub tilladte_ip: String,
    pub vaertsnavn: String,
    pub max_mb: String,
    pub starttls: bool,
    pub tls_motor: smtp_graph::config::TlsMotor,
    pub kraev_login: bool,
    pub login_uden_tls: bool,
    pub brugere: Vec<(String, String)>,
    pub tilladte_modtagere: String,
    pub afsender_navn: String,
    pub svar_til_original: bool,
    pub fejl_til: String,
    pub toerlob: bool,
    pub max_timer: String,
}

impl Formular {
    fn fra(c: &Config) -> Formular {
        Formular {
            lyt: c.smtp.lyt.join(", "),
            lyt_tls: c.smtp.lyt_tls.join(", "),
            tilladte_ip: c.smtp.tilladte_ip.join("\n"),
            vaertsnavn: c.smtp.vaertsnavn.clone(),
            max_mb: c.smtp.max_stoerrelse_mb.to_string(),
            starttls: c.smtp.starttls,
            tls_motor: c.smtp.tls_motor,
            kraev_login: c.smtp.kraev_login,
            login_uden_tls: c.smtp.login_uden_tls,
            brugere: c.smtp.bruger.iter().map(|b| (b.navn.clone(), b.adgangskode.clone())).collect(),
            tilladte_modtagere: c.graph.tilladte_modtagere.join("\n"),
            afsender_navn: c.graph.afsender_navn.clone().unwrap_or_default(),
            svar_til_original: c.graph.svar_til_original,
            fejl_til: c.graph.fejl_til.clone().unwrap_or_default(),
            toerlob: c.graph.toerlob,
            max_timer: c.koe.max_timer.to_string(),
        }
    }
}

/// Microsoft 365-fanen.
#[derive(Default)]
pub struct MsForm {
    pub postkasse: String,
    pub navn: String,
    pub send_som: String,
    pub opret_delt: bool,
    pub kun_send: bool,
    pub forny: bool,
    pub kode: String,
    pub tenant: String,
    pub client_id: String,
    pub secret: String,
    pub m_postkasse: String,
    pub m_from: String,
    pub test_til: String,
    pub test_stor: bool,
    pub vis_hemmelighed: bool,
    /// Valgt metode i fanen: false = admin/app, true = log ind som postkassen.
    pub bruger: bool,
    /// Ved brugerlogin: send fra en anden (delt) postkasse end den, der logger ind.
    pub send_fra: String,
    pub faelles_app: String,
    pub vis_avanceret: bool,
}

enum Bekraeft {
    Slet(Status, String),
    Afinstaller,
}

struct App {
    fane: Fane,
    admin: bool,
    cfg_fil: PathBuf,
    cfg: Option<Config>,
    cfg_fejl: Option<String>,
    form: Formular,
    ms: MsForm,
    tjeneste: Result<Option<ServiceState>, String>,
    sidst: Instant,
    porte: Vec<(String, bool)>,
    koe: Vec<(Status, Post, u64)>,
    koe_filter: Option<Status>,
    log: Vec<String>,
    log_fil: Option<PathBuf>,
    log_filter: String,
    log_kun_fejl: bool,
    besked: Option<(bool, String, Instant)>,
    job: Option<Job>,
    bekraeft: Option<Bekraeft>,
    ips: Vec<String>,
    shot: Option<(PathBuf, Instant, bool)>,
}

impl App {
    fn ny(admin: bool, cfg_fil: Option<PathBuf>) -> App {
        let exe_mappe = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
        let cfg_fil = cfg_fil
            .or_else(service::installeret_config)
            .unwrap_or_else(|| exe_mappe.join("smtp-graph.toml"));
        let mut a = App {
            fane: Fane::Oversigt,
            admin,
            cfg_fil,
            cfg: None,
            cfg_fejl: None,
            form: Formular::default(),
            ms: MsForm { navn: whoami_navn(), ..Default::default() },
            tjeneste: Ok(None),
            sidst: Instant::now() - Duration::from_secs(60),
            porte: vec![],
            koe: vec![],
            koe_filter: None,
            log: vec![],
            log_fil: None,
            log_filter: String::new(),
            log_kun_fejl: false,
            besked: None,
            job: None,
            bekraeft: None,
            ips: lokale_ip(),
            shot: None,
        };
        a.genindlaes();
        a
    }

    fn genindlaes(&mut self) {
        if !self.cfg_fil.exists() {
            self.cfg = None;
            self.cfg_fejl = None;
            return;
        }
        match Config::laes(&self.cfg_fil) {
            Ok(mut c) => {
                self.form = Formular::fra(&c);
                if let Ok(a) = c.graph_adgang() {
                    if let smtp_graph::config::Noegle::Hemmelighed(s) = &a.noegle {
                        self.ms.m_postkasse = a.postkasse.clone();
                        self.ms.m_from = a.from.clone();
                        self.ms.tenant = a.tenant.clone();
                        self.ms.client_id = a.client_id.clone();
                        self.ms.secret = s.clone();
                        if self.ms.postkasse.is_empty() {
                            self.ms.postkasse = a.postkasse.clone();
                        }
                    } else if self.ms.send_fra.is_empty() && c.graph.postkasse.is_some() {
                        self.ms.send_fra = c.graph.postkasse.clone().unwrap_or_default();
                    }
                }
                self.ms.kode = c.graph.kode.clone().unwrap_or_default();
                self.ms.bruger = c.graph.er_bruger();
                if self.ms.faelles_app.is_empty() {
                    self.ms.faelles_app = c.faelles_app();
                }
                self.cfg = Some(c);
                self.cfg_fejl = None;
            }
            Err(e) => {
                self.cfg = None;
                self.cfg_fejl = Some(format!("{e:#}"));
            }
        }
    }

    fn info(&mut self, ok: bool, t: impl Into<String>) {
        self.besked = Some((ok, t.into(), Instant::now()));
    }

    fn mappe(&self) -> PathBuf {
        self.cfg.as_ref().map(|c| c.mappe.clone()).unwrap_or_else(|| self.cfg_fil.parent().map(Path::to_path_buf).unwrap_or_default())
    }

    fn koe_mappe(&self) -> PathBuf {
        self.cfg.as_ref().map(|c| c.koe.mappe.clone()).unwrap_or_else(|| self.mappe().join("koe"))
    }

    /// Opdatér tjeneste, kø og log hvert andet sekund.
    fn poll(&mut self) {
        if self.sidst.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.sidst = Instant::now();
        self.tjeneste = service::tilstand().map_err(|e| format!("{e:#}"));
        self.koe = koe::liste(&self.koe_mappe());
        if let Some(c) = &self.cfg {
            self.porte = c
                .smtp
                .lyt
                .iter()
                .chain(&c.smtp.lyt_tls)
                .map(|a| (a.clone(), port_optaget(a)))
                .collect();
        }
        self.laes_log();
    }

    fn laes_log(&mut self) {
        let d = self.mappe().join("log");
        let nyeste = std::fs::read_dir(&d)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("smtp-graph"))
            .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
        let Some(e) = nyeste else {
            self.log.clear();
            self.log_fil = None;
            return;
        };
        let p = e.path();
        if let Ok(b) = std::fs::read(&p) {
            let start = b.len().saturating_sub(400_000);
            let t = String::from_utf8_lossy(&b[start..]);
            let mut l: Vec<String> = t.lines().map(str::to_string).collect();
            if start > 0 && !l.is_empty() {
                l.remove(0);
            }
            let n = l.len();
            self.log = l.split_off(n.saturating_sub(3000));
        }
        self.log_fil = Some(p);
    }

    fn start_job(&mut self, titel: &str, genindlaes: bool, f: impl FnOnce(&(dyn Fn(String) + Sync)) -> Result<(), String> + Send + 'static) {
        if self.job.as_ref().is_some_and(|j| j.slut.lock().unwrap().is_none()) {
            self.info(false, "Vent til det igangværende job er færdigt.");
            return;
        }
        let linjer = Arc::new(Mutex::new(Vec::new()));
        let slut = Arc::new(Mutex::new(None));
        let (l2, s2) = (linjer.clone(), slut.clone());
        std::thread::spawn(move || {
            let skriv = |t: String| l2.lock().unwrap().push(t);
            let r = f(&skriv);
            *s2.lock().unwrap() = Some(r);
        });
        self.job = Some(Job { titel: titel.into(), linjer, slut, genindlaes });
    }

    fn job_faerdig(&mut self) {
        let Some(j) = &self.job else { return };
        let r = j.slut.lock().unwrap().clone();
        if let Some(r) = r {
            if j.genindlaes {
                self.job.as_mut().unwrap().genindlaes = false;
                self.genindlaes();
                self.sidst = Instant::now() - Duration::from_secs(60);
            }
            if let Err(e) = r {
                if self.besked.as_ref().is_none_or(|b| b.1 != e) {
                    self.info(false, e);
                }
            }
        }
    }

    // ------------------------------------------------------------ tjenesten

    fn tjeneste_knapper(&mut self, ui: &mut egui::Ui) {
        let st = self.tjeneste.clone();
        let (tekst, farve) = match &st {
            Ok(Some(ServiceState::Running)) => ("Kører", GROEN),
            Ok(Some(ServiceState::Stopped)) => ("Stoppet", ROED),
            Ok(Some(ServiceState::StartPending)) => ("Starter …", GUL),
            Ok(Some(ServiceState::StopPending)) => ("Stopper …", GUL),
            Ok(Some(_)) => ("Ukendt tilstand", GUL),
            Ok(None) => ("Ikke installeret", Color32::GRAY),
            Err(_) => ("Kan ikke læse tjenesten", GUL),
        };
        ui.label(RichText::new("Tjeneste:").strong());
        ui.label(RichText::new(tekst).color(farve).strong());
        let travl = self.job.as_ref().is_some_and(|j| j.slut.lock().unwrap().is_none());
        let kan = self.admin && !travl;
        match st {
            Ok(Some(ServiceState::Running)) => {
                if ui.add_enabled(kan, egui::Button::new("Stop")).clicked() {
                    self.start_job("Stopper tjenesten", false, |s| {
                        service::stop().map_err(|e| format!("{e:#}"))?;
                        s("Tjenesten er stoppet.".into());
                        Ok(())
                    });
                }
                if ui.add_enabled(kan, egui::Button::new("Genstart")).clicked() {
                    self.genstart_job();
                }
            }
            Ok(Some(_)) => {
                if ui.add_enabled(kan, egui::Button::new("Start")).clicked() {
                    self.start_job("Starter tjenesten", false, |s| {
                        service::start().map_err(|e| format!("{e:#}"))?;
                        s("Tjenesten er startet.".into());
                        Ok(())
                    });
                }
            }
            _ => {}
        }
        let installeret = matches!(st, Ok(Some(_)));
        let tekst = if installeret { "Geninstallér" } else { "Installér tjeneste" };
        if ui.add_enabled(kan && self.cfg.is_some(), egui::Button::new(tekst)).on_hover_text("Opretter Windows-tjenesten SMTPGraph (automatisk start), åbner firewallen og begrænser adgangen til indstillingsfilen").clicked() {
            self.installer_job();
        }
        if installeret && ui.add_enabled(kan, egui::Button::new("Afinstallér")).clicked() {
            self.bekraeft = Some(Bekraeft::Afinstaller);
        }
    }

    fn genstart_job(&mut self) {
        self.start_job("Genstarter tjenesten", false, |s| {
            service::genstart().map_err(|e| format!("{e:#}"))?;
            s("Tjenesten er genstartet.".into());
            Ok(())
        });
    }

    fn installer_job(&mut self) {
        let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("smtp-graph.exe")));
        let Some(exe) = exe.filter(|e| e.exists()) else {
            self.info(false, "smtp-graph.exe skal ligge i samme mappe som smtp-graph-gui.exe.");
            return;
        };
        let fil = self.cfg_fil.clone();
        self.start_job("Installerer tjenesten", false, move |s| {
            for l in service::installer(&fil, &exe).map_err(|e| format!("{e:#}"))? {
                s(l);
            }
            Ok(())
        });
    }

    // ------------------------------------------------------------ faner

    fn oversigt(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.cfg.clone() else {
            self.ingen_config(ui);
            return;
        };
        let mut c2 = c.clone();
        let adgang = c2.graph_adgang().ok();

        ui.heading("Sådan indstilles scanneren");
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::Grid::new("scanner").num_columns(2).spacing([24.0, 8.0]).striped(true).show(ui, |ui| {
                ui.label(RichText::new("SMTP-server").strong());
                ui.vertical(|ui| {
                    let navn = std::env::var("COMPUTERNAME").unwrap_or_default();
                    for ip in &self.ips {
                        ui.label(RichText::new(ip).monospace().strong());
                    }
                    if !navn.is_empty() {
                        ui.label(RichText::new(format!("eller navnet {navn}")).weak());
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Port og kryptering").strong());
                ui.vertical(|ui| {
                    for (adr, optaget) in &self.porte {
                        let port = adr.rsplit(':').next().unwrap_or(adr);
                        let tls = c.smtp.lyt_tls.contains(adr);
                        let krypt = if tls {
                            "SSL/TLS (fra start)".to_string()
                        } else if c.smtp.starttls {
                            "Ingen eller STARTTLS".to_string()
                        } else {
                            "Ingen".to_string()
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("Port {port}")).monospace().strong());
                            ui.label(format!("– {krypt}"));
                            let kører = matches!(self.tjeneste, Ok(Some(ServiceState::Running)));
                            if *optaget && kører {
                                ui.label(RichText::new("lytter").color(GROEN));
                            } else if *optaget {
                                ui.label(RichText::new("porten bruges af et andet program!").color(ROED));
                            } else {
                                ui.label(RichText::new("lytter ikke").color(GUL));
                            }
                            if !adr.starts_with("0.0.0.0") && !adr.starts_with("[::]") {
                                ui.label(RichText::new(format!("(kun {adr})")).weak());
                            }
                        });
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Login").strong());
                ui.vertical(|ui| {
                    if c.smtp.bruger.is_empty() {
                        ui.label("Intet login (adgang styres af IP-adressen)");
                    } else {
                        let navne: Vec<&str> = c.smtp.bruger.iter().map(|b| b.navn.as_str()).collect();
                        ui.label(format!(
                            "{}: {}",
                            if c.smtp.kraev_login { "Påkrævet" } else { "Valgfrit" },
                            navne.join(", ")
                        ));
                        if !c.smtp.login_uden_tls {
                            ui.label(RichText::new("Kun efter STARTTLS/TLS").weak());
                        }
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Afsenderadresse").strong());
                ui.label(match &adgang {
                    Some(a) => format!("Hvad som helst – mailen sendes som {}", a.from),
                    None => "Hvad som helst".into(),
                });
                ui.end_row();

                ui.label(RichText::new("Må forbinde fra").strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label(if c.smtp.tilladte_ip.is_empty() {
                        RichText::new("Ingen! Tilføj scannerens IP").color(ROED)
                    } else {
                        RichText::new(c.smtp.tilladte_ip.join(", "))
                    });
                    if ui.link("Ret").on_hover_text("Rettes under Modtager (SMTP)").clicked() {
                        self.fane = Fane::Modtager;
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Modtagere").strong());
                ui.horizontal_wrapped(|ui| {
                    ui.label(if c.graph.tilladte_modtagere.is_empty() {
                        "Alle".to_string()
                    } else {
                        format!("Kun {}", c.graph.tilladte_modtagere.join(", "))
                    });
                    if ui.link("Ret").on_hover_text("Rettes under Modtager (SMTP)").clicked() {
                        self.fane = Fane::Modtager;
                    }
                });
                ui.end_row();

                ui.label(RichText::new("Største mail").strong());
                ui.label(format!("{} MB", c.smtp.max_stoerrelse_mb));
                ui.end_row();

                if c.smtp.starttls || !c.smtp.lyt_tls.is_empty() {
                    ui.label(RichText::new("TLS-certifikat").strong());
                    ui.label(match &c.smtp.tls_cert {
                        Some(p) => p.display().to_string(),
                        None => format!("Selvsigneret ({})", smtp_graph::tls::auto_filer(&c.mappe).0.display()),
                    });
                    ui.end_row();
                    ui.label(RichText::new("TLS-motor").strong());
                    ui.label(motor_tekst(c.smtp.tls_motor));
                    ui.end_row();
                }
            });
        });
        if c.graph.toerlob {
            ui.colored_label(GUL, "⚠ Tørløb er slået til: mails gemmes i koe\\toerlob og sendes IKKE.");
        }

        ui.add_space(10.0);
        ui.heading("Microsoft 365");
        egui::Frame::group(ui.style()).show(ui, |ui| match &adgang {
            Some(a) => {
                egui::Grid::new("ms").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                    ui.label(RichText::new("Sender fra").strong());
                    ui.label(&a.from);
                    ui.end_row();
                    ui.label(RichText::new("Postkasse").strong());
                    ui.label(&a.postkasse);
                    ui.end_row();
                    ui.label(RichText::new("Adgang").strong());
                    ui.label(a.konto_tekst());
                    ui.end_row();
                    if let Some(u) = &c2.graph.udloeber {
                        ui.label(RichText::new("Hemmelighed udløber").strong());
                        let dage = chrono::NaiveDate::parse_from_str(u, "%Y-%m-%d")
                            .map(|d| (d - chrono::Local::now().date_naive()).num_days())
                            .unwrap_or(999);
                        let farve = if dage < 0 { ROED } else if dage < 30 { GUL } else { GROEN };
                        ui.label(RichText::new(format!("{u} (om {dage} dage)")).color(farve));
                        ui.end_row();
                    }
                });
            }
            None => {
                ui.colored_label(GUL, "Ikke sat op endnu.");
                if ui.button("Sæt Microsoft 365 op …").clicked() {
                    self.fane = Fane::Microsoft;
                }
            }
        });

        ui.add_space(10.0);
        ui.heading("Kø");
        let venter = self.koe.iter().filter(|k| k.0 == Status::Venter).count();
        let fejlet = self.koe.iter().filter(|k| k.0 == Status::Fejlet).count();
        ui.horizontal(|ui| {
            ui.label(format!("{venter} venter"));
            ui.label("·");
            let t = RichText::new(format!("{fejlet} opgivet"));
            ui.label(if fejlet > 0 { t.color(ROED) } else { t });
            if ui.link("Vis køen").clicked() {
                self.fane = Fane::Koe;
            }
        });

        let fejlfil = c.mappe.join("smtp-graph-fejl.txt");
        if let Ok(t) = std::fs::read_to_string(&fejlfil) {
            ui.add_space(10.0);
            ui.colored_label(ROED, "Tjenesten stoppede sidst med en fejl:");
            ui.label(RichText::new(t.trim()).monospace());
        }

        ui.add_space(10.0);
        ui.heading("Seneste i loggen");
        let sidste: Vec<String> = self.log.iter().rev().take(8).rev().cloned().collect();
        if sidste.is_empty() {
            ui.label(RichText::new("Ingen log endnu.").weak());
        }
        for l in sidste {
            log_linje(ui, &l);
        }
    }

    fn ingen_config(&mut self, ui: &mut egui::Ui) {
        ui.add_space(20.0);
        if let Some(f) = &self.cfg_fejl {
            ui.colored_label(ROED, "Indstillingsfilen kan ikke læses:");
            ui.label(RichText::new(f).monospace());
            ui.label(self.cfg_fil.display().to_string());
            if ui.button("Åbn filen i Notesblok").clicked() {
                let _ = std::process::Command::new("notepad").arg(&self.cfg_fil).spawn();
            }
            if ui.button("Læs igen").clicked() {
                self.genindlaes();
            }
            return;
        }
        ui.heading("Velkommen til SMTP-Graph");
        ui.label(format!("Der er ingen indstillinger endnu i {}.", self.cfg_fil.display()));
        if ui.button("Opret indstillinger").clicked() {
            match std::fs::write(&self.cfg_fil, SKABELON) {
                Ok(_) => {
                    self.genindlaes();
                    self.info(true, "Indstillingerne er oprettet. Ret Modtager-fanen og sæt Microsoft 365 op.");
                }
                Err(e) => self.info(false, format!("Kan ikke skrive {}: {e}", self.cfg_fil.display())),
            }
        }
    }

    fn microsoft(&mut self, ui: &mut egui::Ui) {
        let Some(mut c) = self.cfg.clone() else {
            self.ingen_config(ui);
            return;
        };
        let travl = self.job.as_ref().is_some_and(|j| j.slut.lock().unwrap().is_none());
        let har_faelles = !self.ms.faelles_app.trim().is_empty();

        ui.heading("Nu");
        egui::Frame::group(ui.style()).show(ui, |ui| match c.graph_adgang() {
            Ok(a) => {
                ui.label(format!("Sender som {} gennem postkassen {}.", a.from, a.postkasse));
                let metode = if c.graph.er_bruger() { "Logget ind som bruger" } else { "Kundens egen app" };
                ui.label(RichText::new(format!("{metode}: {}", a.konto_tekst())).weak());
                if let smtp_graph::config::Noegle::Login(f) = &a.noegle {
                    if let Ok(l) = smtp_graph::oauth::Login::laes(f) {
                        ui.label(RichText::new(format!("Login sidst fornyet {}", l.fornyet.with_timezone(&chrono::Local).format("%d-%m-%Y %H:%M"))).weak());
                    }
                }
            }
            Err(e) => {
                ui.colored_label(GUL, format!("Ikke sat op: {e:#}"));
            }
        });

        ui.add_space(10.0);
        ui.heading("Sæt op hos kunden");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.ms.bruger, false, RichText::new("Administrator opretter app (anbefalet)").size(15.0));
            ui.selectable_value(&mut self.ms.bruger, true, RichText::new("Log ind som postkassen").size(15.0));
        });
        egui::Frame::group(ui.style()).show(ui, |ui| {
            if !self.ms.bruger {
                ui.label("Log ind som administrator i kundens Microsoft 365 (global administrator, eller program- og Exchange-administrator). Programmet opretter kundens egen app, som kun må sende fra den ene postkasse. Ingen PowerShell. Hemmeligheden gælder 2 år.");
                ui.add_space(4.0);
                egui::Grid::new("admin").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("Postkasse");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.postkasse).hint_text("scanner@kunde.dk").desired_width(320.0));
                    ui.end_row();
                    ui.label("Stedets navn");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.navn).hint_text("kunde.dk kontor").desired_width(320.0))
                        .on_hover_text("Bruges i appens navn hos kunden, så den er let at finde");
                    ui.end_row();
                    ui.label("Send som (valgfri)");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.send_som).hint_text("en anden adresse, postkassen må sende som").desired_width(320.0));
                    ui.end_row();
                    ui.label("");
                    ui.vertical(|ui| {
                        ui.checkbox(&mut self.ms.opret_delt, "Opret postkassen som delt postkasse, hvis den ikke findes");
                        ui.checkbox(&mut self.ms.kun_send, "Kun Mail.Send (ingen mails over ca. 3 MB)");
                    });
                    ui.end_row();
                });
                let ok = self.ms.postkasse.contains('@') && !self.ms.navn.trim().is_empty() && !travl && har_faelles;
                if ui.add_enabled(ok, egui::Button::new(RichText::new("Log ind som administrator og sæt op").strong())).clicked() {
                    self.admin_job();
                }
            } else {
                ui.label("Log ind med den konto, der skal sende (fx scanner@kunde.dk med licens). SMTP-Graph gemmer login og fornyer det selv hver dag. Skifter kontoen adgangskode, eller spærres login, skal du logge ind igen her.");
                ui.label(RichText::new("En delt postkasse kan ikke selv logge ind. Log i så fald ind med en bruger, der har »Send som« og fuld adgang til den, og skriv den delte postkasse herunder.").weak());
                ui.add_space(4.0);
                egui::Grid::new("brugerlogin").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("Send fra (valgfri)");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.send_fra).hint_text("tom = kontoen, der logger ind").desired_width(320.0));
                    ui.end_row();
                });
                if ui.add_enabled(!travl && har_faelles, egui::Button::new(RichText::new("Log ind med Microsoft").strong())).clicked() {
                    self.bruger_job();
                }
            }
            if !har_faelles {
                ui.colored_label(GUL, "Irvolds fælles login-app er ikke sat endnu – se Avanceret herunder.");
            }
        });

        ui.add_space(10.0);
        ui.heading("Afprøv");
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Prøv login hos Microsoft").clicked() {
                    self.test_job(None);
                }
                ui.separator();
                ui.label("Testmail til");
                ui.add(egui::TextEdit::singleline(&mut self.ms.test_til).hint_text("dig@kunde.dk").desired_width(240.0));
                ui.checkbox(&mut self.ms.test_stor, "med 5 MB vedhæftning");
                if ui.add_enabled(self.ms.test_til.contains('@'), egui::Button::new("Send")).clicked() {
                    self.test_job(Some((self.ms.test_til.trim().to_string(), self.ms.test_stor)));
                }
            });
            ui.label(RichText::new("Testen bruger de gemte indstillinger og går direkte til Microsoft (ikke gennem køen). Nye rettigheder i Exchange kan tage op til 2 timer.").weak());
        });

        ui.add_space(10.0);
        egui::CollapsingHeader::new("Avanceret: fælles login-app, PowerShell og manuel opsætning")
            .default_open(!har_faelles)
            .show(ui, |ui| {
                ui.label(RichText::new("Irvolds fælles login-app").strong());
                ui.label(RichText::new("Multi-tenant app i Irvolds Microsoft 365, som alle kunder logger ind gennem. Oprettes én gang med Opret-FaellesApp.ps1.").weak());
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.ms.faelles_app).hint_text("app-id (GUID)").desired_width(320.0));
                    if ui.button("Gem").clicked() {
                        match gem::gem_faelles(&self.cfg_fil, &self.ms.faelles_app) {
                            Ok(_) => {
                                self.genindlaes();
                                self.info(true, "Den fælles app er gemt.");
                            }
                            Err(e) => self.info(false, e),
                        }
                    }
                });

                ui.add_space(8.0);
                ui.label(RichText::new("Opsætning med PowerShell").strong());
                ui.label(RichText::new("Samme som administrator-opsætningen, men gennem Microsofts PowerShell-moduler (kræver PowerShell 7, browseren åbner to gange). Bruger felterne ovenfor.").weak());
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.ms.forny, "Forny kun hemmeligheden");
                    let ok = self.ms.postkasse.contains('@') && !self.ms.navn.trim().is_empty() && !travl;
                    if ui.add_enabled(ok, egui::Button::new("Kør opsætning med PowerShell")).clicked() {
                        self.koer_script();
                    }
                });

                ui.add_space(8.0);
                ui.label(RichText::new("Manuelt").strong());
                egui::Grid::new("man").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("Opsætningskode");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.kode).hint_text("SMTPGRAPH1:… eller IRVMAIL1:…").desired_width(520.0).password(!self.ms.vis_hemmelighed));
                    ui.end_row();
                    ui.label("");
                    ui.label(RichText::new("… eller felterne hver for sig (bruges kun, når koden er tom):").weak());
                    ui.end_row();
                    ui.label("Tenant-id");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.tenant).desired_width(520.0));
                    ui.end_row();
                    ui.label("App-id (klient)");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.client_id).desired_width(520.0));
                    ui.end_row();
                    ui.label("Klienthemmelighed");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.secret).desired_width(520.0).password(!self.ms.vis_hemmelighed));
                    ui.end_row();
                    ui.label("Postkasse");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.m_postkasse).desired_width(520.0));
                    ui.end_row();
                    ui.label("Afsender");
                    ui.add(egui::TextEdit::singleline(&mut self.ms.m_from).hint_text("tom = postkassen").desired_width(520.0));
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.ms.vis_hemmelighed, "Vis hemmeligheder");
                    if ui.button("Gem manuel opsætning").clicked() {
                        match gem::gem_ms(&self.cfg_fil, &self.ms) {
                            Ok(_) => {
                                self.genindlaes();
                                self.info(true, "Microsoft 365-opsætningen er gemt. Genstart tjenesten for at bruge den.");
                            }
                            Err(e) => self.info(false, e),
                        }
                    }
                });
            });
    }

    fn efter_opsaetning(s: &(dyn Fn(String) + Sync)) {
        let kører = matches!(service::tilstand(), Ok(Some(ServiceState::Running)));
        if kører {
            s("Genstarter tjenesten, så den bruger den nye opsætning …".into());
            match service::genstart() {
                Ok(_) => s("Tjenesten er genstartet.".into()),
                Err(e) => s(format!("FEJL: kunne ikke genstarte tjenesten: {e:#}")),
            }
        }
    }

    fn admin_job(&mut self) {
        let valg = smtp_graph::opsaet::Valg {
            postkasse: self.ms.postkasse.trim().to_string(),
            navn: self.ms.navn.trim().to_string(),
            send_som: Some(self.ms.send_som.trim().to_string()).filter(|s| !s.is_empty()),
            opret_delt: self.ms.opret_delt,
            kun_send: self.ms.kun_send,
            maaneder: 24,
        };
        let faelles = self.ms.faelles_app.trim().to_string();
        let fil = self.cfg_fil.clone();
        self.start_job("Opsætning hos kunden", true, move |s| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            let r = rt.block_on(smtp_graph::opsaet::admin(&faelles, &valg, s)).map_err(|e| format!("{e:#}"))?;
            gem::gem_app_kode(&fil, &r.kode)?;
            s(format!("Gemt: app {} sender fra {}. Hemmeligheden udløber {}.", r.app_id, r.postkasse, r.udloeber));
            s("Nye rettigheder i Exchange kan tage op til 2 timer. Imens venter mails i køen.".into());
            Self::efter_opsaetning(s);
            Ok(())
        });
    }

    fn bruger_job(&mut self) {
        let faelles = self.ms.faelles_app.trim().to_string();
        let send_fra = self.ms.send_fra.trim().to_lowercase();
        let fil = self.cfg_fil.clone();
        self.start_job("Log ind med Microsoft", true, move |s| {
            let cfg = Config::laes(&fil).map_err(|e| format!("{e:#}"))?;
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            let (login, mail) = rt.block_on(smtp_graph::opsaet::bruger(&faelles, s)).map_err(|e| format!("{e:#}"))?;
            login.gem(&cfg.login_fil()).map_err(|e| format!("kan ikke gemme login: {e:#}"))?;
            gem::gem_bruger(&fil, &send_fra)?;
            let fra = if send_fra.is_empty() { mail } else { send_fra };
            s(format!("Gemt: SMTP-Graph sender nu som {fra} (logget ind som {}).", login.konto));
            Self::efter_opsaetning(s);
            Ok(())
        });
    }

    fn koer_script(&mut self) {
        let pwsh = ["C:\\Program Files\\PowerShell\\7\\pwsh.exe", "C:\\Program Files\\PowerShell\\7-preview\\pwsh.exe"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.exists())
            .or_else(|| {
                std::process::Command::new("where").arg("pwsh").output().ok().and_then(|o| {
                    String::from_utf8_lossy(&o.stdout).lines().next().map(|l| PathBuf::from(l.trim())).filter(|p| p.exists())
                })
            });
        let Some(pwsh) = pwsh else {
            self.info(false, "PowerShell 7 (pwsh) mangler. Installér det med: winget install Microsoft.PowerShell");
            return;
        };
        let script = std::env::temp_dir().join("Opsaet-SmtpGraph.ps1");
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(SCRIPT.as_bytes());
        if let Err(e) = std::fs::write(&script, b) {
            self.info(false, format!("Kan ikke skrive scriptet: {e}"));
            return;
        }
        let mut args: Vec<String> = vec![
            "-NoProfile".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            script.display().to_string(),
            "-Postkasse".into(),
            self.ms.postkasse.trim().into(),
            "-Navn".into(),
            self.ms.navn.trim().into(),
            "-Config".into(),
            self.cfg_fil.display().to_string(),
        ];
        if !self.ms.send_som.trim().is_empty() {
            args.extend(["-SendSom".into(), self.ms.send_som.trim().into()]);
        }
        for (b, n) in [(self.ms.opret_delt, "-OpretDelt"), (self.ms.kun_send, "-KunSend"), (self.ms.forny, "-Forny")] {
            if b {
                args.push(n.into());
            }
        }
        self.start_job("Opsætning i Microsoft 365", true, move |s| {
            use std::io::BufRead;
            use std::process::{Command, Stdio};
            let mut cmd = Command::new(&pwsh);
            cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            s("Starter PowerShell … (browseren åbner om lidt)".into());
            let mut barn = cmd.spawn().map_err(|e| format!("kan ikke starte pwsh: {e}"))?;
            let fejl = barn.stderr.take().unwrap();
            let fejl_linjer = Arc::new(Mutex::new(Vec::<String>::new()));
            let fl = fejl_linjer.clone();
            let t = std::thread::spawn(move || {
                for l in std::io::BufReader::new(fejl).lines().map_while(Result::ok) {
                    fl.lock().unwrap().push(uden_ansi(&l));
                }
            });
            for l in std::io::BufReader::new(barn.stdout.take().unwrap()).lines().map_while(Result::ok) {
                s(uden_ansi(&l));
                for f in fejl_linjer.lock().unwrap().drain(..) {
                    s(format!("FEJL: {f}"));
                }
            }
            let st = barn.wait().map_err(|e| e.to_string())?;
            let _ = t.join();
            for f in fejl_linjer.lock().unwrap().drain(..) {
                s(format!("FEJL: {f}"));
            }
            let _ = std::fs::remove_file(&script);
            if st.success() {
                s("Færdig. Genstart tjenesten, så den bruger den nye opsætning.".into());
                Ok(())
            } else {
                Err("Opsætningen i Microsoft 365 fejlede – se teksten i jobvinduet.".into())
            }
        });
    }

    fn test_job(&mut self, mail: Option<(String, bool)>) {
        let fil = self.cfg_fil.clone();
        let titel = if mail.is_some() { "Sender testmail" } else { "Prøver login" };
        self.start_job(titel, false, move |s| {
            let mut cfg = Config::laes(&fil).map_err(|e| format!("{e:#}"))?;
            let a = cfg.graph_adgang().map_err(|e| format!("{e:#}"))?;
            s(format!("Postkasse {}, afsender {}", a.postkasse, a.from));
            let g = smtp_graph::graph::Graph::ny(a, cfg.graph.login_url.clone(), cfg.graph.graph_url.clone());
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
            rt.block_on(async {
                g.token().await.map_err(|e| e.tekst)?;
                s("Login hos Microsoft: OK".into());
                if let Some((til, stor)) = mail {
                    let raw = smtp_graph::testmail(&til, stor);
                    let vej = g
                        .send(&raw, "", &[til.clone()], cfg.graph.afsender_navn.as_deref(), false)
                        .await
                        .map_err(|e| e.tekst)?;
                    s(format!("Testmail sendt til {til} ({vej}, {} kB)", raw.len() / 1024));
                }
                Ok(())
            })
        });
    }

    fn modtager(&mut self, ui: &mut egui::Ui) {
        if self.cfg.is_none() {
            self.ingen_config(ui);
            return;
        }
        let f = &mut self.form;
        ui.heading("Modtageserveren (SMTP)");
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::Grid::new("smtp").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                ui.label("Lyt på");
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut f.lyt).desired_width(360.0));
                    ui.label(RichText::new("Fx 0.0.0.0:25, 0.0.0.0:587 – adskil med komma").weak());
                });
                ui.end_row();
                ui.label("Lyt med TLS fra start");
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut f.lyt_tls).hint_text("fx 0.0.0.0:465").desired_width(360.0));
                    ui.label(RichText::new("Til scannere, hvor kryptering hedder SSL/TLS. Tom = slået fra").weak());
                });
                ui.end_row();
                ui.label("Tilbyd STARTTLS");
                ui.checkbox(&mut f.starttls, "(selvsigneret certifikat, hvis intet andet er sat)");
                ui.end_row();
                ui.label("TLS-motor");
                ui.vertical(|ui| {
                    use smtp_graph::config::TlsMotor;
                    egui::ComboBox::from_id_salt("tls_motor")
                        .selected_text(motor_tekst(f.tls_motor))
                        .width(360.0)
                        .show_ui(ui, |ui| {
                            for m in [TlsMotor::Auto, TlsMotor::Rustls, TlsMotor::Windows] {
                                ui.selectable_value(&mut f.tls_motor, m, motor_tekst(m));
                            }
                        });
                    ui.label(
                        RichText::new("Automatisk klarer også ældre scannere, der ellers melder \"ingen sikker forbindelse\"")
                            .weak(),
                    );
                });
                ui.end_row();
                ui.label("Må forbinde fra");
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::multiline(&mut f.tilladte_ip).desired_rows(4).desired_width(360.0));
                    ui.label(RichText::new("Én pr. linje: IP-adresse eller net (172.29.3.50 eller 172.29.3.0/24)").weak());
                });
                ui.end_row();
                ui.label("Værtsnavn");
                ui.add(egui::TextEdit::singleline(&mut f.vaertsnavn).desired_width(200.0));
                ui.end_row();
                ui.label("Største mail (MB)");
                ui.add(egui::TextEdit::singleline(&mut f.max_mb).desired_width(80.0));
                ui.end_row();
            });
        });

        ui.add_space(8.0);
        ui.heading("Login");
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.checkbox(&mut f.kraev_login, "Kræv login (ellers er login valgfrit)");
            ui.checkbox(&mut f.login_uden_tls, "Tillad login uden kryptering (mange scannere kan ikke andet)");
            let mut slet = None;
            egui::Grid::new("brugere").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
                if !f.brugere.is_empty() {
                    ui.label(RichText::new("Brugernavn").strong());
                    ui.label(RichText::new("Adgangskode").strong());
                    ui.label("");
                    ui.end_row();
                }
                for (i, (n, k)) in f.brugere.iter_mut().enumerate() {
                    ui.add_sized([180.0, 22.0], egui::TextEdit::singleline(n));
                    ui.add_sized([220.0, 22.0], egui::TextEdit::singleline(k));
                    if ui.button("Fjern").clicked() {
                        slet = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = slet {
                f.brugere.remove(i);
            }
            if ui.button("+ Tilføj bruger").clicked() {
                f.brugere.push(("scanner".into(), tilfaeldig_kode()));
            }
        });

        ui.add_space(8.0);
        ui.heading("Afsendelse");
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::Grid::new("afs").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
                ui.label("Tilladte modtagere");
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::multiline(&mut f.tilladte_modtagere).desired_rows(3).desired_width(360.0));
                    ui.label(RichText::new("Domæner (kunde.dk) eller adresser, én pr. linje. Tom = alle").weak());
                });
                ui.end_row();
                ui.label("Afsendernavn");
                ui.add(egui::TextEdit::singleline(&mut f.afsender_navn).hint_text("fx Scanner – Exchange bruger ofte postkassens navn").desired_width(360.0));
                ui.end_row();
                ui.label("Svar til");
                ui.checkbox(&mut f.svar_til_original, "Sæt scannerens egen afsender som Reply-To");
                ui.end_row();
                ui.label("Besked ved fejl til");
                ui.add(egui::TextEdit::singleline(&mut f.fejl_til).hint_text("it@kunde.dk").desired_width(360.0));
                ui.end_row();
                ui.label("Opgiv efter (timer)");
                ui.add(egui::TextEdit::singleline(&mut f.max_timer).desired_width(80.0));
                ui.end_row();
                ui.label("Tørløb");
                ui.checkbox(&mut f.toerlob, "Send intet – gem mails i koe\\toerlob (til afprøvning)");
                ui.end_row();
            });
        });

        ui.add_space(8.0);
        let kører = matches!(self.tjeneste, Ok(Some(ServiceState::Running)));
        ui.horizontal(|ui| {
            let gem = ui.button(RichText::new("Gem").strong()).clicked();
            let gem_gen = kører && ui.add_enabled(self.admin, egui::Button::new("Gem og genstart tjenesten")).clicked();
            if ui.button("Fortryd").clicked() {
                self.genindlaes();
            }
            if gem || gem_gen {
                match gem::gem_smtp(&self.cfg_fil, &self.form) {
                    Ok(_) => {
                        self.genindlaes();
                        if gem_gen {
                            self.genstart_job();
                        } else {
                            self.info(true, if kører { "Gemt. Genstart tjenesten for at bruge ændringerne." } else { "Gemt." });
                        }
                    }
                    Err(e) => self.info(false, e),
                }
            }
        });
    }

    fn koe_fane(&mut self, ui: &mut egui::Ui) {
        let mappe = self.koe_mappe();
        ui.horizontal(|ui| {
            ui.heading("Kø");
            ui.add_space(12.0);
            for (f, t) in [(None, "Alle"), (Some(Status::Venter), "Venter"), (Some(Status::Fejlet), "Opgivet"), (Some(Status::Toerlob), "Tørløb")] {
                let n = self.koe.iter().filter(|k| f.is_none_or(|f| k.0 == f)).count();
                ui.selectable_value(&mut self.koe_filter, f, format!("{t} ({n})"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("📂 Åbn mappen").clicked() {
                    let _ = std::process::Command::new("explorer").arg(&mappe).spawn();
                }
            });
        });
        ui.label(RichText::new(mappe.display().to_string()).weak());
        ui.separator();
        let liste: Vec<_> = self.koe.iter().filter(|k| self.koe_filter.is_none_or(|f| k.0 == f)).cloned().collect();
        if liste.is_empty() {
            ui.add_space(20.0);
            ui.label("Køen er tom. Mails, der er sendt, forsvinder herfra.");
            return;
        }
        let mut handling: Option<(&'static str, Status, Post)> = None;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("koe").num_columns(8).spacing([14.0, 6.0]).striped(true).show(ui, |ui| {
                for h in ["", "Status", "Modtaget", "Fra", "Til", "Størrelse", "Forsøg", "Næste / fejl"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (st, p, str) in &liste {
                    ui.horizontal(|ui| {
                        if ui.small_button("Send nu").on_hover_text("Prøv at sende igen med det samme").clicked() {
                            handling = Some(("igen", *st, p.clone()));
                        }
                        if ui.small_button("Åbn").on_hover_text("Åbn mailen (.eml)").clicked() {
                            handling = Some(("aabn", *st, p.clone()));
                        }
                        if ui.small_button("Slet").clicked() {
                            handling = Some(("slet", *st, p.clone()));
                        }
                    });
                    let farve = match st {
                        Status::Venter => GUL,
                        Status::Fejlet => ROED,
                        Status::Toerlob => Color32::GRAY,
                    };
                    ui.label(RichText::new(st.tekst()).color(farve).strong());
                    ui.label(p.modtaget.with_timezone(&chrono::Local).format("%d-%m %H:%M:%S").to_string()).on_hover_text(&p.id);
                    ui.label(if p.fra.is_empty() { "<>" } else { &p.fra }).on_hover_text(format!("Afsendt af {}", p.klient));
                    ui.label(p.til.join(", "));
                    ui.label(format!("{:.1} MB", *str as f64 / 1_048_576.0));
                    ui.label(p.forsoeg.to_string());
                    let naeste = if *st == Status::Venter && p.forsoeg > 0 {
                        format!("igen {}", p.naeste.with_timezone(&chrono::Local).format("%H:%M"))
                    } else {
                        String::new()
                    };
                    let fejl = p.sidste_fejl.clone().unwrap_or_default();
                    let kort: String = fejl.chars().take(70).collect();
                    let r = ui.label(format!("{naeste}{}{kort}", if naeste.is_empty() || kort.is_empty() { "" } else { " – " }));
                    if !fejl.is_empty() {
                        r.on_hover_text(&fejl);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some((h, st, p)) = handling {
            match h {
                "igen" => match koe::proev_igen(&mappe, st, p) {
                    Ok(_) => {
                        self.info(true, "Lagt i kø til afsendelse (inden for 20 sekunder).");
                        self.sidst = Instant::now() - Duration::from_secs(60);
                    }
                    Err(e) => self.info(false, format!("Kunne ikke: {e:#}")),
                },
                "aabn" => {
                    let _ = std::process::Command::new("explorer").arg(koe::eml_sti(&mappe, st, &p.id)).spawn();
                }
                _ => self.bekraeft = Some(Bekraeft::Slet(st, p.id)),
            }
        }
    }

    fn om_fane(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.label(RichText::new("SMTP-Graph").size(28.0).strong());
        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).size(15.0));
        ui.add_space(8.0);
        ui.label("Tager imod mail fra scannere og printere (SMTP) og sender dem videre gennem Microsoft 365 (Microsoft Graph).");
        ui.add_space(20.0);
        ui.label(RichText::new("Udviklet af").weak());
        ui.label(RichText::new("Irvold IT & Kommunikation").size(17.0).strong());
        ui.label("4750 Lundby");
        if ui.link(RichText::new("www.irvold.dk").size(15.0)).on_hover_text("https://www.irvold.dk").clicked() {
            aabn_adresse("https://www.irvold.dk");
        }
    }

    fn log_fane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Log");
            ui.add_space(12.0);
            ui.label("Søg");
            ui.add(egui::TextEdit::singleline(&mut self.log_filter).desired_width(220.0));
            ui.checkbox(&mut self.log_kun_fejl, "Kun advarsler og fejl");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("📂 Åbn logmappen").clicked() {
                    let _ = std::process::Command::new("explorer").arg(self.mappe().join("log")).spawn();
                }
            });
        });
        if let Some(f) = &self.log_fil {
            ui.label(RichText::new(f.display().to_string()).weak());
        }
        ui.separator();
        let filt = self.log_filter.to_lowercase();
        let linjer: Vec<&String> = self
            .log
            .iter()
            .filter(|l| !self.log_kun_fejl || l.contains(" WARN ") || l.contains(" ERROR "))
            .filter(|l| filt.is_empty() || l.to_lowercase().contains(&filt))
            .collect();
        egui::ScrollArea::both().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            if linjer.is_empty() {
                ui.label(RichText::new("Ingen linjer.").weak());
            }
            for l in linjer {
                log_linje(ui, l);
            }
        });
    }

    fn job_vindue(&mut self, ctx: &egui::Context) {
        let Some(j) = &self.job else { return };
        let faerdig = j.slut.lock().unwrap().clone();
        let mut aaben = true;
        let mut luk = false;
        egui::Window::new(&j.titel)
            .collapsible(false)
            .resizable(true)
            .default_size([640.0, 360.0])
            .open(&mut aaben)
            .show(ctx, |ui| {
                let linjer = j.linjer.lock().unwrap().clone();
                egui::ScrollArea::vertical().max_height(360.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                    for l in &linjer {
                        let t = RichText::new(l).monospace();
                        ui.label(if l.starts_with("FEJL") { t.color(ROED) } else { t });
                    }
                });
                ui.separator();
                match &faerdig {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Arbejder …");
                        });
                    }
                    Some(Ok(())) => {
                        ui.horizontal(|ui| {
                            ui.colored_label(GROEN, "Færdig");
                            if ui.button("Luk").clicked() {
                                luk = true;
                            }
                        });
                    }
                    Some(Err(e)) => {
                        ui.colored_label(ROED, format!("Fejl: {e}"));
                        if ui.button("Luk").clicked() {
                            luk = true;
                        }
                    }
                }
            });
        if luk || (!aaben && faerdig.is_some()) {
            self.job = None;
        }
    }

    fn bekraeft_vindue(&mut self, ctx: &egui::Context) {
        let Some(b) = &self.bekraeft else { return };
        let tekst = match b {
            Bekraeft::Slet(_, id) => format!("Slet mailen {id} for altid? Den bliver ikke sendt."),
            Bekraeft::Afinstaller => "Fjern Windows-tjenesten og firewall-reglen? Indstillinger, kø og log bliver liggende.".into(),
        };
        let mut svar = None;
        egui::Modal::new(egui::Id::unique("bekraeft")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(tekst);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Ja").color(ROED)).clicked() {
                    svar = Some(true);
                }
                if ui.button("Annullér").clicked() {
                    svar = Some(false);
                }
            });
        });
        if let Some(ja) = svar {
            let b = self.bekraeft.take().unwrap();
            if ja {
                match b {
                    Bekraeft::Slet(st, id) => match koe::slet(&self.koe_mappe(), st, &id) {
                        Ok(_) => {
                            self.info(true, format!("{id} er slettet."));
                            self.sidst = Instant::now() - Duration::from_secs(60);
                        }
                        Err(e) => self.info(false, format!("Kunne ikke slette: {e}")),
                    },
                    Bekraeft::Afinstaller => self.start_job("Afinstallerer tjenesten", false, |s| {
                        s(service::afinstaller().map_err(|e| format!("{e:#}"))?);
                        Ok(())
                    }),
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.job_faerdig();
        ctx.request_repaint_after(Duration::from_millis(if self.job.is_some() { 150 } else { 1000 }));

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("SMTP-Graph").size(20.0).strong());
                ui.label(RichText::new(env!("CARGO_PKG_VERSION")).weak());
                ui.separator();
                self.tjeneste_knapper(ui);
            });
            if !self.admin {
                ui.colored_label(GUL, "⚠ Kører uden administratorrettigheder: tjenesten kan ikke styres, og en installeret opsætning kan ikke læses.");
            }
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                for (f, t) in [
                    (Fane::Oversigt, "Oversigt"),
                    (Fane::Microsoft, "Microsoft 365"),
                    (Fane::Modtager, "Modtager (SMTP)"),
                    (Fane::Koe, "Kø"),
                    (Fane::Log, "Log"),
                    (Fane::Om, "Om"),
                ] {
                    let mut t = t.to_string();
                    if f == Fane::Koe {
                        let n = self.koe.iter().filter(|k| k.0 != Status::Toerlob).count();
                        if n > 0 {
                            t = format!("Kø ({n})");
                        }
                    }
                    ui.selectable_value(&mut self.fane, f, RichText::new(t).size(15.0));
                }
            });
            ui.add_space(2.0);
        });
        egui::Panel::bottom("bund").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.cfg_fil.display().to_string()).weak()).on_hover_text("Indstillingsfilen");
                if let Some((ok, t, tid)) = &self.besked {
                    if tid.elapsed() < Duration::from_secs(20) {
                        ui.separator();
                        ui.colored_label(if *ok { GROEN } else { ROED }, t);
                    }
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            match self.fane {
                Fane::Oversigt | Fane::Microsoft | Fane::Modtager => {
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.fane {
                        Fane::Oversigt => self.oversigt(ui),
                        Fane::Microsoft => self.microsoft(ui),
                        _ => self.modtager(ui),
                    });
                }
                Fane::Koe => self.koe_fane(ui),
                Fane::Log => self.log_fane(ui),
                Fane::Om => self.om_fane(ui),
            }
        });
        self.job_vindue(&ctx);
        self.bekraeft_vindue(&ctx);

        if let Some((sti, t0, spurgt)) = &mut self.shot {
            if !*spurgt && t0.elapsed() > Duration::from_secs(3) {
                *spurgt = true;
                let sti = sti.clone();
                ctx.request_screenshot(move |img| {
                    let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    if let Some(b) = image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, rgba) {
                        let _ = b.save(&sti);
                    }
                    std::process::exit(0);
                });
            }
        }
    }
}

fn log_linje(ui: &mut egui::Ui, l: &str) {
    let t = RichText::new(l).monospace().size(12.5);
    ui.label(if l.contains(" ERROR ") {
        t.color(ROED)
    } else if l.contains(" WARN ") {
        t.color(GUL)
    } else {
        t
    });
}

/// Er porten i brug? (Prøver at binde den et øjeblik.)
fn port_optaget(adr: &str) -> bool {
    let a = adr.trim();
    matches!(std::net::TcpListener::bind(a), Err(e) if e.kind() == std::io::ErrorKind::AddrInUse || e.raw_os_error() == Some(10013))
}

fn lokale_ip() -> Vec<String> {
    let mut v: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(v4) if !v4.is_link_local() => Some(v4.to_string()),
            _ => None,
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

fn whoami_navn() -> String {
    std::env::var("COMPUTERNAME").map(|n| n.to_lowercase()).unwrap_or_default()
}

fn tilfaeldig_kode() -> String {
    let u = uuid::Uuid::new_v4().simple().to_string();
    u[..16].to_string()
}

fn uden_ansi(s: &str) -> String {
    let mut ud = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                while let Some(&n) = it.peek() {
                    it.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        ud.push(c);
    }
    ud
}

#[cfg(test)]
mod gui_test {
    use super::*;
    use egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    fn harness(fane: Fane) -> (Harness<'static, App>, PathBuf) {
        let d = std::env::temp_dir().join(format!("smtpgraph-gui-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        let fil = d.join("smtp-graph.toml");
        std::fs::write(&fil, SKABELON).unwrap();
        let f2 = fil.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1120.0, 780.0)).with_accessibility_check(false).build_eframe(move |_cc| {
            let mut a = App::ny(true, Some(f2));
            a.fane = fane;
            a
        });
        for _ in 0..3 {
            h.step();
        }
        (h, fil)
    }

    #[test]
    fn kan_skrive_i_tilladte_ip() {
        let (mut h, _) = harness(Fane::Modtager);
        let n = h.get_all_by_role(Role::MultilineTextInput).count();
        eprintln!("multiline-felter: {n}");
        h.get_all_by_role(Role::MultilineTextInput).next().unwrap().click();
        h.step();
        h.get_all_by_role(Role::MultilineTextInput).next().unwrap().type_text("X");
        h.step();
        h.step();
        let f = &h.state().form;
        eprintln!("lyt={:?}\nip={:?}", f.lyt, f.tilladte_ip);
        assert!(f.tilladte_ip.contains('X'), "teksten kom ikke i feltet: {:?} / lyt {:?}", f.tilladte_ip, f.lyt);
    }
}

fn motor_tekst(m: smtp_graph::config::TlsMotor) -> &'static str {
    use smtp_graph::config::TlsMotor;
    match m {
        TlsMotor::Auto => "Automatisk (moderne TLS + Windows til ældre scannere)",
        TlsMotor::Rustls => "Kun moderne TLS (rustls)",
        TlsMotor::Windows => "Kun Windows' TLS (Schannel)",
    }
}
