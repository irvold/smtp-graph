//! Linux: systemd-tjeneste, systembruger og (hvis ufw er slået til) firewall.
//!
//! Programmet ligger i /usr/local/bin/smtp-graph (ejet af root), indstillinger, kø, log og
//! certifikat i /var/lib/smtp-graph (ejet af systembrugeren smtp-graph). Tjenesten kører som
//! smtp-graph med kun CAP_NET_BIND_SERVICE, så den kan lytte på port 25.

use crate::config::Config;
use anyhow::{bail, Context, Result};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const NAVN: &str = "smtp-graph";
pub const BRUGER: &str = "smtp-graph";
pub const MAPPE: &str = "/var/lib/smtp-graph";
pub const PROGRAM: &str = "/usr/local/bin/smtp-graph";
const UNIT: &str = "/etc/systemd/system/smtp-graph.service";
const UFW_MAERKE: &str = "smtp-graph";

/// Standardindstillingsfilen på Linux.
pub fn std_config() -> PathBuf {
    Path::new(MAPPE).join("smtp-graph.toml")
}

fn er_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn koer(kmd: &str, args: &[&str]) -> Result<std::process::Output> {
    Command::new(kmd).args(args).output().with_context(|| format!("kunne ikke køre {kmd}"))
}

fn ok(kmd: &str, args: &[&str]) -> bool {
    koer(kmd, args).map(|o| o.status.success()).unwrap_or(false)
}

fn skal(kmd: &str, args: &[&str]) -> Result<()> {
    let o = koer(kmd, args)?;
    if !o.status.success() {
        bail!("{kmd} {} fejlede: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim());
    }
    Ok(())
}

/// Kører programmet som root, og ejes indstillingsmappen af en anden (tjenestens bruger),
/// skifter vi til den bruger. Så får nye filer (log, kø, login) den rigtige ejer, og
/// `sudo smtp-graph tjek` kan ikke ødelægge rettighederne for tjenesten.
pub fn som_ejer(fil: &Path) -> Result<()> {
    if !er_root() {
        return Ok(());
    }
    let mappe = match fil.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Ok(md) = std::fs::metadata(&mappe) else { return Ok(()) };
    if md.uid() == 0 {
        return Ok(());
    }
    unsafe {
        if libc::setgroups(0, std::ptr::null()) != 0 || libc::setgid(md.gid()) != 0 || libc::setuid(md.uid()) != 0 {
            bail!("kunne ikke skifte til ejeren af {} (uid {})", mappe.display(), md.uid());
        }
    }
    Ok(())
}

/// Installér (eller opdatér) programmet og tjenesten. Kan køres igen, fx når Microsoft 365 er sat op.
pub fn installer(fil: &Path) -> Result<Vec<String>> {
    if !er_root() {
        bail!("kør som root: sudo {} installer", std::env::current_exe().map(|p| p.display().to_string()).unwrap_or(NAVN.into()));
    }
    if !Path::new("/run/systemd/system").exists() {
        bail!("systemd kører ikke på denne maskine – start i stedet `smtp-graph tjeneste` med jeres egen init");
    }
    let mut ud = Vec::new();
    let fil = std::path::absolute(fil)?;
    let mappe = fil.parent().context("indstillingsfilen har ingen mappe")?.to_path_buf();

    // 1. Systembruger
    if !ok("id", &["-u", BRUGER]) {
        let skal_ = ["/usr/sbin/nologin", "/sbin/nologin", "/bin/false"].into_iter().find(|p| Path::new(p).exists()).unwrap_or("/bin/false");
        skal("useradd", &["--system", "--user-group", "--no-create-home", "--home-dir", &mappe.to_string_lossy(), "--shell", skal_, BRUGER])?;
        ud.push(format!("Systembrugeren {BRUGER} er oprettet."));
    }

    // 2. Programmet til /usr/local/bin (via midlertidig fil, så et kørende program kan erstattes)
    let exe = std::env::current_exe()?.canonicalize()?;
    let maal = PathBuf::from(PROGRAM);
    if maal.canonicalize().ok().as_deref() != Some(exe.as_path()) {
        let tmp = maal.with_extension("ny");
        std::fs::copy(&exe, &tmp).with_context(|| format!("kunne ikke kopiere til {}", tmp.display()))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, &maal)?;
        ud.push(format!("Programmet er kopieret til {PROGRAM}."));
    }

    // 3. Mappe og indstillinger
    std::fs::create_dir_all(&mappe)?;
    if !fil.exists() {
        std::fs::write(&fil, crate::config::SKABELON)?;
        ud.push(format!("Indstillingsfilen {} er oprettet.", fil.display()));
    }
    let ejer = format!("{BRUGER}:{BRUGER}");
    skal("chown", &["-R", &ejer, &mappe.to_string_lossy()])?;
    std::fs::set_permissions(&mappe, std::fs::Permissions::from_mode(0o750))?;
    std::fs::set_permissions(&fil, std::fs::Permissions::from_mode(0o600))?;

    let cfg = Config::laes(&fil);
    let mut skriv = vec![mappe.clone()];
    if let Ok(c) = &cfg {
        let login = c.login_fil();
        if login.exists() {
            std::fs::set_permissions(&login, std::fs::Permissions::from_mode(0o600))?;
        }
        // Ligger køen uden for mappen, skal tjenesten også kunne skrive der.
        if !c.koe.mappe.starts_with(&mappe) {
            std::fs::create_dir_all(&c.koe.mappe)?;
            skal("chown", &["-R", &ejer, &c.koe.mappe.to_string_lossy()])?;
            skriv.push(c.koe.mappe.clone());
        }
    }

    // 4. systemd
    let skriv: Vec<String> = skriv.iter().map(|p| p.display().to_string()).collect();
    let unit = format!(
        "[Unit]\n\
         Description=SMTP-Graph (mail fra scannere sendes gennem Microsoft Graph)\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         User={BRUGER}\n\
         Group={BRUGER}\n\
         ExecStart={PROGRAM} --config {fil} tjeneste\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         AmbientCapabilities=CAP_NET_BIND_SERVICE\n\
         CapabilityBoundingSet=CAP_NET_BIND_SERVICE\n\
         NoNewPrivileges=true\n\
         ProtectSystem=strict\n\
         ProtectHome=true\n\
         PrivateTmp=true\n\
         PrivateDevices=true\n\
         ReadWritePaths={rw}\n\
         UMask=0077\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        fil = fil.display(),
        rw = skriv.join(" ")
    );
    std::fs::write(UNIT, unit)?;
    skal("systemctl", &["daemon-reload"])?;
    ud.push(format!("Tjenesten {NAVN} er oprettet ({UNIT})."));

    // 5. Firewall og start – kun når indstillingerne er gyldige
    let cfg = match cfg {
        Ok(c) => c,
        Err(e) => {
            ud.push(format!("Indstillingerne er ikke gyldige endnu ({e:#}). Ret {} og kør installer igen.", fil.display()));
            return Ok(ud);
        }
    };
    ud.push(firewall(&cfg));
    let mut c2 = cfg.clone();
    if let Err(e) = c2.graph_adgang() {
        ud.push(format!("Microsoft 365 er ikke sat op endnu ({e:#}), så tjenesten er ikke startet."));
        ud.push(format!("Kør:  sudo smtp-graph opsaet --postkasse scanner@firma.dk --navn \"Firma kontor\" --opret-delt"));
        ud.push("og derefter:  sudo smtp-graph installer".into());
        return Ok(ud);
    }
    // Først nu slås automatisk start til – ellers ville en tjeneste uden adgang genstarte i ring.
    skal("systemctl", &["enable", NAVN])?;
    skal("systemctl", &["restart", NAVN])?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    if ok("systemctl", &["is-active", "--quiet", NAVN]) {
        ud.push(format!("Tjenesten kører og starter automatisk. Log: {}  (eller: journalctl -u {NAVN} -f)", cfg.mappe.join("log").display()));
    } else {
        ud.push(format!("Advarsel: tjenesten startede ikke. Se: journalctl -u {NAVN} -n 50"));
    }
    Ok(ud)
}

fn ufw_aktiv() -> bool {
    koer("ufw", &["status"]).map(|o| String::from_utf8_lossy(&o.stdout).contains("Status: active")).unwrap_or(false)
}

/// Fjern vores ufw-regler (de er mærket med kommentaren smtp-graph).
fn ufw_fjern() {
    let Ok(o) = koer("ufw", &["status", "numbered"]) else { return };
    let mut numre: Vec<u32> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| l.trim_end().ends_with(&format!("# {UFW_MAERKE}")))
        .filter_map(|l| l.trim_start().strip_prefix('[')?.split(']').next()?.trim().parse().ok())
        .collect();
    numre.sort_unstable_by(|a, b| b.cmp(a));
    for n in numre {
        let _ = koer("ufw", &["--force", "delete", &n.to_string()]);
    }
}

/// Åbn ufw for portene fra tilladte_ip – kun hvis ufw er slået til.
pub fn firewall(cfg: &Config) -> String {
    let porte: Vec<String> =
        cfg.smtp.lyt.iter().chain(&cfg.smtp.lyt_tls).filter_map(|a| a.rsplit(':').next().map(str::to_string)).collect();
    if !ufw_aktiv() {
        return format!(
            "Firewall: ufw er ikke slået til. Bruger I en anden firewall, så åbn TCP {} fra scannerens adresse.",
            porte.join(",")
        );
    }
    ufw_fjern();
    let ip: Vec<&str> = cfg.smtp.tilladte_ip.iter().map(|s| s.trim()).filter(|s| !s.starts_with("127.") && *s != "::1").collect();
    let mut fejl = 0;
    for p in &porte {
        if ip.is_empty() {
            fejl += !ok("ufw", &["allow", "proto", "tcp", "to", "any", "port", p, "comment", UFW_MAERKE]) as u32;
        }
        for i in &ip {
            fejl += !ok("ufw", &["allow", "proto", "tcp", "from", i, "to", "any", "port", p, "comment", UFW_MAERKE]) as u32;
        }
    }
    if fejl > 0 {
        "Advarsel: nogle ufw-regler kunne ikke oprettes.".into()
    } else {
        format!("Firewall (ufw) åbnet for TCP {} fra {}.", porte.join(","), if ip.is_empty() { "alle".into() } else { ip.join(", ") })
    }
}

pub fn afinstaller() -> Result<String> {
    if !er_root() {
        bail!("kør som root: sudo smtp-graph afinstaller");
    }
    if !Path::new(UNIT).exists() {
        bail!("tjenesten {NAVN} er ikke installeret");
    }
    let _ = koer("systemctl", &["disable", "--now", NAVN]);
    std::fs::remove_file(UNIT)?;
    let _ = koer("systemctl", &["daemon-reload"]);
    if ufw_aktiv() {
        ufw_fjern();
    }
    Ok(format!(
        "Tjenesten {NAVN} og firewall-reglerne er fjernet. Indstillinger, kø og log i {MAPPE} er ikke rørt.\n\
         Alt fjernes med:  sudo rm -r {MAPPE} {PROGRAM} && sudo userdel {BRUGER}"
    ))
}
