//! Gem ændringer i smtp-graph.toml uden at miste kommentarerne (toml_edit).

use crate::{Formular, MsForm};
use smtp_graph::config::{Config, SKABELON};
use std::path::Path;
use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};

fn laes(fil: &Path) -> Result<DocumentMut, String> {
    let tekst = std::fs::read_to_string(fil).unwrap_or_else(|_| SKABELON.to_string());
    tekst.parse::<DocumentMut>().map_err(|e| format!("Indstillingsfilen kan ikke læses: {e}"))
}

fn skriv(fil: &Path, doc: &DocumentMut) -> Result<(), String> {
    let tekst = doc.to_string();
    // Tjek, at resultatet er gyldigt, før det skrives.
    Config::fra_tekst(&tekst, fil).map_err(|e| format!("Ikke gemt: {e:#}"))?;
    std::fs::write(fil, tekst).map_err(|e| format!("Kan ikke skrive {}: {e}", fil.display()))
}

fn tabel<'a>(doc: &'a mut DocumentMut, navn: &str) -> &'a mut Table {
    if !doc.contains_table(navn) {
        doc[navn] = Item::Table(Table::new());
    }
    doc[navn].as_table_mut().unwrap()
}

fn liste(s: &str) -> Array {
    s.split([',', '\n', ';'])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect()
}

fn tekst_eller_fjern(t: &mut Table, k: &str, v: &str) {
    if v.trim().is_empty() {
        t.remove(k);
    } else {
        t[k] = value(v.trim());
    }
}

pub fn gem_smtp(fil: &Path, f: &Formular) -> Result<(), String> {
    let mut doc = laes(fil)?;
    let tal = |s: &str, hvad: &str| s.trim().parse::<i64>().map_err(|_| format!("{hvad} skal være et tal"));
    let max_mb = tal(&f.max_mb, "Største mail")?;
    let max_timer = tal(&f.max_timer, "Opgiv efter")?;
    for (n, k) in &f.brugere {
        if n.trim().is_empty() || k.is_empty() {
            return Err("Alle brugere skal have navn og adgangskode".into());
        }
    }
    {
        let s = tabel(&mut doc, "smtp");
        s["lyt"] = value(liste(&f.lyt));
        s["lyt_tls"] = value(liste(&f.lyt_tls));
        s["tilladte_ip"] = value(liste(&f.tilladte_ip));
        s["vaertsnavn"] = value(f.vaertsnavn.trim());
        s["max_stoerrelse_mb"] = value(max_mb);
        s["starttls"] = value(f.starttls);
        s["tls_motor"] = value(f.tls_motor.navn());
        s["kraev_login"] = value(f.kraev_login);
        s["login_uden_tls"] = value(f.login_uden_tls);
        if f.brugere.is_empty() {
            s.remove("bruger");
        } else {
            let mut aot = ArrayOfTables::new();
            for (n, k) in &f.brugere {
                let mut t = Table::new();
                t["navn"] = value(n.trim());
                t["adgangskode"] = value(k.as_str());
                aot.push(t);
            }
            s["bruger"] = Item::ArrayOfTables(aot);
        }
    }
    {
        let g = tabel(&mut doc, "graph");
        g["tilladte_modtagere"] = value(liste(&f.tilladte_modtagere));
        tekst_eller_fjern(g, "afsender_navn", &f.afsender_navn);
        g["svar_til_original"] = value(f.svar_til_original);
        tekst_eller_fjern(g, "fejl_til", &f.fejl_til);
        g["toerlob"] = value(f.toerlob);
    }
    tabel(&mut doc, "koe")["max_timer"] = value(max_timer);
    skriv(fil, &doc)
}

pub fn gem_app_kode(fil: &Path, kode: &str) -> Result<(), String> {
    smtp_graph::skriv::app_kode(fil, kode).map_err(|e| format!("{e:#}"))
}

pub fn gem_bruger(fil: &Path, send_fra: &str) -> Result<(), String> {
    smtp_graph::skriv::bruger(fil, send_fra).map_err(|e| format!("{e:#}"))
}

pub fn gem_faelles(fil: &Path, app: &str) -> Result<(), String> {
    let app = app.trim();
    if !app.is_empty() && app.len() != 36 {
        return Err("App-id'et skal være en GUID (36 tegn)".into());
    }
    let mut doc = laes(fil)?;
    tekst_eller_fjern(tabel(&mut doc, "graph"), "faelles_app", app);
    skriv(fil, &doc)
}

pub fn gem_ms(fil: &Path, m: &MsForm) -> Result<(), String> {
    let mut doc = laes(fil)?;
    let g = tabel(&mut doc, "graph");
    g.remove("metode");
    if !m.kode.trim().is_empty() {
        g["kode"] = value(m.kode.trim());
        for k in ["tenant", "client_id", "secret", "postkasse", "from"] {
            g.remove(k);
        }
    } else {
        if [&m.tenant, &m.client_id, &m.secret, &m.m_postkasse].iter().any(|v| v.trim().is_empty()) {
            return Err("Udfyld opsætningskoden – eller tenant, app-id, hemmelighed og postkasse".into());
        }
        g["kode"] = value("");
        g["tenant"] = value(m.tenant.trim());
        g["client_id"] = value(m.client_id.trim());
        g["secret"] = value(m.secret.trim());
        g["postkasse"] = value(m.m_postkasse.trim());
        tekst_eller_fjern(g, "from", &m.m_from);
    }
    skriv(fil, &doc)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn gemmer_og_bevarer_kommentarer() {
        let d = std::env::temp_dir().join(format!("smtpgraph-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&d).unwrap();
        let fil = d.join("smtp-graph.toml");
        std::fs::write(&fil, SKABELON).unwrap();
        let mut f = crate::Formular::fra(&Config::laes(&fil).unwrap());
        f.lyt = "0.0.0.0:25, 0.0.0.0:587".into();
        f.tilladte_ip = "172.29.3.50\n172.29.4.0/24".into();
        f.brugere = vec![("scanner".into(), "hemmelig".into())];
        f.kraev_login = true;
        f.fejl_til = "it@firma.dk".into();
        gem_smtp(&fil, &f).unwrap();
        let mut m = crate::MsForm::default();
        m.tenant = "t".into();
        m.client_id = "c".into();
        m.secret = "s".into();
        m.m_postkasse = "scanner@firma.dk".into();
        gem_ms(&fil, &m).unwrap();
        let tekst = std::fs::read_to_string(&fil).unwrap();
        assert!(tekst.contains("# Kun disse net må aflevere mail"), "{tekst}");
        let mut c = Config::laes(&fil).unwrap();
        assert_eq!(c.smtp.lyt, vec!["0.0.0.0:25", "0.0.0.0:587"]);
        assert_eq!(c.smtp.tilladte_ip, vec!["172.29.3.50", "172.29.4.0/24"]);
        assert_eq!(c.smtp.bruger.len(), 1);
        assert!(c.smtp.kraev_login);
        assert_eq!(c.graph.fejl_til.as_deref(), Some("it@firma.dk"));
        assert_eq!(c.graph_adgang().unwrap().postkasse, "scanner@firma.dk");
        // Ugyldige værdier gemmes ikke.
        f.tilladte_ip = "ikke-en-ip".into();
        assert!(gem_smtp(&fil, &f).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
