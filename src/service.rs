//! Windows-tjeneste: installér, fjern, styr og kør.

use crate::config::Config;
use anyhow::{Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use windows_service::service::{
    Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo,
    ServiceStartType, ServiceStatus, ServiceType,
};
pub use windows_service::service::ServiceState;
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

pub const NAVN: &str = "SMTPGraph";
const VIST: &str = "SMTP-Graph (scanner-mail via Microsoft 365)";
const FIREWALL: &str = "SMTP-Graph";

static CONFIG: OnceLock<PathBuf> = OnceLock::new();

define_windows_service!(ffi_main, tjeneste_main);

/// Kaldes, når Windows' tjenestestyring starter programmet.
pub fn dispatch(fil: PathBuf) -> Result<()> {
    let _ = CONFIG.set(fil);
    service_dispatcher::start(NAVN, ffi_main).context("kan kun startes af Windows' tjenestestyring")?;
    Ok(())
}

fn tjeneste_main(_args: Vec<OsString>) {
    let _ = koer();
}

fn status(s: ServiceState, kode: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: s,
        controls_accepted: if s == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(kode),
        checkpoint: 0,
        wait_hint: Duration::from_secs(20),
        process_id: None,
    }
}

fn koer() -> Result<()> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let handle = service_control_handler::register(NAVN, move |c| match c {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = tx.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(ServiceState::StartPending, 0))?;
    let fil = CONFIG.get().cloned().unwrap_or_else(crate::std_config);
    let r = (|| -> Result<()> {
        let cfg = Config::laes(&fil)?;
        let _g = crate::log(&cfg, false);
        handle.set_service_status(status(ServiceState::Running, 0))?;
        let rt = tokio::runtime::Runtime::new()?;
        let r = rt.block_on(crate::koer(cfg, rx));
        if let Err(e) = &r {
            tracing::error!("tjenesten stoppede med fejl: {e:#}");
        }
        r
    })();
    let fejlfil = fil.with_file_name("smtp-graph-fejl.txt");
    match &r {
        // Uden log (fx fejl i config) skrives fejlen ved siden af config-filen.
        Err(e) => {
            let _ = std::fs::write(&fejlfil, format!("{}\r\n{e:#}\r\n", chrono::Local::now()));
        }
        Ok(()) => {
            let _ = std::fs::remove_file(&fejlfil);
        }
    }
    handle.set_service_status(status(ServiceState::Stopped, if r.is_ok() { 0 } else { 1 }))?;
    Ok(())
}

// ---------------------------------------------------------------- styring

fn aabn(adgang: ServiceAccess) -> Result<Option<Service>> {
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    match mgr.open_service(NAVN, adgang) {
        Ok(s) => Ok(Some(s)),
        Err(windows_service::Error::Winapi(e)) if e.raw_os_error() == Some(1060) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Tjenestens tilstand, eller None hvis den ikke er installeret.
pub fn tilstand() -> Result<Option<ServiceState>> {
    Ok(match aabn(ServiceAccess::QUERY_STATUS)? {
        Some(s) => Some(s.query_status()?.current_state),
        None => None,
    })
}

/// Den indstillingsfil, den installerede tjeneste bruger.
pub fn installeret_config() -> Option<PathBuf> {
    let s = aabn(ServiceAccess::QUERY_CONFIG).ok()??;
    let kmd = s.query_config().ok()?.executable_path.to_string_lossy().into_owned();
    let (_, rest) = kmd.split_once("--config")?;
    let rest = rest.trim_start();
    let sti = if let Some(r) = rest.strip_prefix('"') {
        r.split('"').next()?
    } else {
        rest.split_whitespace().next()?
    };
    Some(PathBuf::from(sti))
}

/// Det program, tjenesten kører.
pub fn installeret_exe() -> Option<PathBuf> {
    let s = aabn(ServiceAccess::QUERY_CONFIG).ok()??;
    let kmd = s.query_config().ok()?.executable_path.to_string_lossy().into_owned();
    let exe = match kmd.strip_prefix('"') {
        Some(r) => r.split('"').next()?.to_string(),
        None => kmd.split(" --config").next()?.trim().to_string(),
    };
    Some(PathBuf::from(exe))
}

pub fn start() -> Result<()> {
    let s = aabn(ServiceAccess::START | ServiceAccess::QUERY_STATUS)?.context("tjenesten er ikke installeret")?;
    if s.query_status()?.current_state != ServiceState::Running {
        s.start(&[] as &[&OsStr])?;
        vent(&s, ServiceState::Running);
    }
    Ok(())
}

pub fn stop() -> Result<()> {
    let s = aabn(ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)?.context("tjenesten er ikke installeret")?;
    if s.query_status()?.current_state != ServiceState::Stopped {
        s.stop()?;
        vent(&s, ServiceState::Stopped);
    }
    Ok(())
}

pub fn genstart() -> Result<()> {
    stop()?;
    start()
}

fn vent(svc: &Service, s: ServiceState) {
    for _ in 0..80 {
        if svc.query_status().map(|x| x.current_state == s).unwrap_or(false) {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Installér (eller opdatér) tjenesten, så den kører `exe` med indstillingerne i `fil`.
pub fn installer(fil: &Path, exe: &Path) -> Result<Vec<String>> {
    let mut ud = Vec::new();
    let fil = std::path::absolute(fil)?;
    let cfg = Config::laes(&fil).context("ret indstillingerne, før tjenesten installeres")?;
    let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
        .context("kør som administrator")?;
    let info = ServiceInfo {
        name: NAVN.into(),
        display_name: VIST.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.to_path_buf(),
        launch_arguments: vec!["--config".into(), fil.clone().into_os_string(), "tjeneste".into()],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let adgang = ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS | ServiceAccess::STOP;
    let svc = match mgr.open_service(NAVN, adgang) {
        Ok(s) => {
            s.change_config(&info)?;
            ud.push(format!("Tjenesten {NAVN} fandtes og er opdateret."));
            s
        }
        Err(_) => {
            let s = mgr.create_service(&info, adgang)?;
            ud.push(format!("Tjenesten {NAVN} er oprettet."));
            s
        }
    };
    let _ = svc.set_description("Tager imod mail fra scannere (SMTP) og sender dem gennem Microsoft Graph.");
    let genstart = ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(10) };
    svc.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![genstart.clone(), genstart.clone(), genstart]),
    })?;
    let _ = svc.set_failure_actions_on_non_crash_failures(true);

    // Kun SYSTEM og administratorer må læse indstillingerne (der står en hemmelighed i dem).
    let ok = std::process::Command::new("icacls")
        .arg(&fil)
        .args(["/inheritance:r", "/grant:r", "*S-1-5-18:F", "*S-1-5-32-544:F"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    ud.push(if ok { "Adgangen til indstillingsfilen er begrænset til SYSTEM og administratorer.".into() } else { "Advarsel: kunne ikke begrænse adgangen til indstillingsfilen.".into() });

    ud.push(firewall(&cfg, exe));

    if svc.query_status()?.current_state == ServiceState::Running {
        svc.stop()?;
        vent(&svc, ServiceState::Stopped);
    }
    svc.start(&[] as &[&OsStr])?;
    vent(&svc, ServiceState::Running);
    ud.push(format!("Tjenesten kører. Log: {}", cfg.mappe.join("log").display()));
    Ok(ud)
}

/// Firewall: kun de porte, vi lytter på, og kun fra tilladte_ip.
pub fn firewall(cfg: &Config, exe: &Path) -> String {
    let porte: Vec<String> =
        cfg.smtp.lyt.iter().chain(&cfg.smtp.lyt_tls).filter_map(|a| a.rsplit(':').next().map(str::to_string)).collect();
    let _ = std::process::Command::new("netsh").args(["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL}")]).output();
    let mut fw = std::process::Command::new("netsh");
    fw.args(["advfirewall", "firewall", "add", "rule", &format!("name={FIREWALL}"), "dir=in", "action=allow", "protocol=TCP"])
        .arg(format!("localport={}", porte.join(",")))
        .arg(format!("program={}", exe.display()));
    let ip: Vec<&str> = cfg.smtp.tilladte_ip.iter().map(|s| s.trim()).filter(|s| !s.starts_with("127.") && *s != "::1").collect();
    if !ip.is_empty() {
        fw.arg(format!("remoteip={}", ip.join(",")));
    }
    let ok = fw.output().map(|o| o.status.success()).unwrap_or(false);
    if ok {
        format!("Firewall åbnet for TCP {} fra {}.", porte.join(","), if ip.is_empty() { "alle".into() } else { ip.join(", ") })
    } else {
        "Advarsel: firewall-reglen kunne ikke oprettes.".into()
    }
}

pub fn afinstaller() -> Result<String> {
    let svc = aabn(ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE)?
        .context("tjenesten er ikke installeret")?;
    if svc.query_status()?.current_state != ServiceState::Stopped {
        let _ = svc.stop();
        vent(&svc, ServiceState::Stopped);
    }
    svc.delete()?;
    let _ = std::process::Command::new("netsh").args(["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL}")]).output();
    Ok(format!("Tjenesten {NAVN} og firewall-reglen er fjernet. Køen og loggen er ikke rørt."))
}
