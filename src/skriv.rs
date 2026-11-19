//! Skriv Microsoft 365-opsætningen ind i smtp-graph.toml uden at miste kommentarerne.

use crate::config::{Config, SKABELON};
use anyhow::{Context, Result};
use std::path::Path;
use toml_edit::{value, DocumentMut, Item, Table};

pub fn laes(fil: &Path) -> Result<DocumentMut> {
    let tekst = std::fs::read_to_string(fil).unwrap_or_else(|_| SKABELON.to_string());
    tekst.parse::<DocumentMut>().context("indstillingsfilen kan ikke læses")
}

/// Tjek resultatet og skriv det (på stedet, så filens adgangsrettigheder bevares).
pub fn skriv(fil: &Path, doc: &DocumentMut) -> Result<()> {
    let tekst = doc.to_string();
    Config::fra_tekst(&tekst, fil).context("ikke gemt")?;
    std::fs::write(fil, tekst).with_context(|| format!("kan ikke skrive {}", fil.display()))
}

pub fn graph(doc: &mut DocumentMut) -> &mut Table {
    if !doc.contains_table("graph") {
        doc["graph"] = Item::Table(Table::new());
    }
    doc["graph"].as_table_mut().unwrap()
}

/// Resultatet af administrator-opsætningen: kundens egen app (metode "app").
pub fn app_kode(fil: &Path, kode: &str) -> Result<()> {
    let mut doc = laes(fil)?;
    let g = graph(&mut doc);
    g["kode"] = value(kode);
    for k in ["tenant", "client_id", "secret", "postkasse", "from", "metode"] {
        g.remove(k);
    }
    skriv(fil, &doc)
}

/// Metoden "bruger": login ligger i graph-login.json; `send_fra` er evt. en delt postkasse.
pub fn bruger(fil: &Path, send_fra: &str) -> Result<()> {
    let mut doc = laes(fil)?;
    let g = graph(&mut doc);
    g["metode"] = value("bruger");
    for k in ["postkasse", "from"] {
        g.remove(k);
    }
    if !send_fra.trim().is_empty() {
        g["postkasse"] = value(send_fra.trim());
    }
    skriv(fil, &doc)
}
