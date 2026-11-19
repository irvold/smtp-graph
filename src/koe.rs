//! Diskkø: hver mail er <id>.eml + <id>.json. Overlever genstart og Graph-nedbrud.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub fra: String,
    pub til: Vec<String>,
    pub klient: String,
    pub modtaget: DateTime<Utc>,
    pub forsoeg: u32,
    pub naeste: DateTime<Utc>,
    pub sidste_fejl: Option<String>,
}

pub struct Koe {
    pub mappe: PathBuf,
}

impl Koe {
    pub fn ny(mappe: &Path) -> Result<Koe> {
        for d in ["", "fejlet", "toerlob"] {
            std::fs::create_dir_all(mappe.join(d))
                .with_context(|| format!("kan ikke oprette {}", mappe.join(d).display()))?;
        }
        Ok(Koe { mappe: mappe.to_path_buf() })
    }

    pub fn eml(&self, id: &str) -> PathBuf { self.mappe.join(format!("{id}.eml")) }
    fn meta(&self, id: &str) -> PathBuf { self.mappe.join(format!("{id}.json")) }

    /// Læg en mail i køen. Først når denne er gået godt, svarer vi 250 til scanneren.
    pub async fn laeg(&self, fra: &str, til: &[String], klient: &str, data: &[u8]) -> Result<Post> {
        let nu = Utc::now();
        let id = format!("{}-{}", nu.format("%Y%m%d-%H%M%S"), &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let post = Post {
            id: id.clone(),
            fra: fra.into(),
            til: til.to_vec(),
            klient: klient.into(),
            modtaget: nu,
            forsoeg: 0,
            naeste: nu,
            sidste_fejl: None,
        };
        skriv_atomisk(&self.eml(&id), data).await?;
        self.gem(&post).await?;
        Ok(post)
    }

    pub async fn gem(&self, post: &Post) -> Result<()> {
        skriv_atomisk(&self.meta(&post.id), &serde_json::to_vec_pretty(post)?).await
    }

    /// Poster, der er klar til (endnu et) forsøg, ældste først.
    pub async fn klar(&self) -> Result<Vec<Post>> {
        let mut ud = Vec::new();
        let mut rd = tokio::fs::read_dir(&self.mappe).await?;
        let nu = Utc::now();
        while let Some(e) = rd.next_entry().await? {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            match tokio::fs::read(&p).await.ok().and_then(|b| serde_json::from_slice::<Post>(&b).ok()) {
                Some(post) if post.naeste <= nu => ud.push(post),
                Some(_) => {}
                None => tracing::warn!("kan ikke læse {}", p.display()),
            }
        }
        ud.sort_by_key(|p| p.modtaget);
        Ok(ud)
    }

    pub async fn antal(&self) -> usize {
        let Ok(mut rd) = tokio::fs::read_dir(&self.mappe).await else { return 0 };
        let mut n = 0;
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.path().extension().and_then(|x| x.to_str()) == Some("json") {
                n += 1;
            }
        }
        n
    }

    pub async fn faerdig(&self, id: &str) {
        let _ = tokio::fs::remove_file(self.meta(id)).await;
        let _ = tokio::fs::remove_file(self.eml(id)).await;
    }

    /// Flyt til en undermappe (fejlet / toerlob).
    pub async fn flyt(&self, post: &Post, under: &str) -> Result<()> {
        let d = self.mappe.join(under);
        tokio::fs::rename(self.eml(&post.id), d.join(format!("{}.eml", post.id))).await?;
        skriv_atomisk(&d.join(format!("{}.json", post.id)), &serde_json::to_vec_pretty(post)?).await?;
        let _ = tokio::fs::remove_file(self.meta(&post.id)).await;
        Ok(())
    }

    /// Ventetid før næste forsøg.
    pub fn pause(forsoeg: u32) -> Duration {
        let min = [1, 2, 5, 10, 30];
        Duration::minutes(*min.get(forsoeg.saturating_sub(1) as usize).unwrap_or(&60))
    }
}

// ---------------------------------------------------------------- til GUI'en (synkront)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Venter,
    Fejlet,
    Toerlob,
}

impl Status {
    pub fn mappe(self) -> &'static str {
        match self {
            Status::Venter => "",
            Status::Fejlet => "fejlet",
            Status::Toerlob => "toerlob",
        }
    }
    pub fn tekst(self) -> &'static str {
        match self {
            Status::Venter => "Venter",
            Status::Fejlet => "Fejlet",
            Status::Toerlob => "Tørløb",
        }
    }
}

/// Alle poster i køen og undermapperne, nyeste først.
pub fn liste(mappe: &Path) -> Vec<(Status, Post, u64)> {
    let mut ud = Vec::new();
    for st in [Status::Venter, Status::Fejlet, Status::Toerlob] {
        let Ok(rd) = std::fs::read_dir(mappe.join(st.mappe())) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            if let Some(post) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Post>(&b).ok()) {
                let str = std::fs::metadata(p.with_extension("eml")).map(|m| m.len()).unwrap_or(0);
                ud.push((st, post, str));
            }
        }
    }
    ud.sort_by(|a, b| b.1.modtaget.cmp(&a.1.modtaget));
    ud
}

pub fn eml_sti(mappe: &Path, st: Status, id: &str) -> PathBuf {
    mappe.join(st.mappe()).join(format!("{id}.eml"))
}

/// Send igen hurtigst muligt (en opgivet mail flyttes tilbage i køen).
pub fn proev_igen(mappe: &Path, st: Status, mut post: Post) -> Result<()> {
    let d = mappe.join(st.mappe());
    if st != Status::Venter {
        std::fs::rename(d.join(format!("{}.eml", post.id)), mappe.join(format!("{}.eml", post.id)))?;
        let _ = std::fs::remove_file(d.join(format!("{}.json", post.id)));
        post.forsoeg = 0;
        // Så den ikke straks opgives igen pga. alder.
        post.modtaget = Utc::now();
    }
    post.naeste = Utc::now();
    let fil = mappe.join(format!("{}.json", post.id));
    let tmp = fil.with_extension("gui-tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&post)?)?;
    std::fs::rename(&tmp, &fil)?;
    Ok(())
}

pub fn slet(mappe: &Path, st: Status, id: &str) -> Result<()> {
    let d = mappe.join(st.mappe());
    std::fs::remove_file(d.join(format!("{id}.json")))?;
    let _ = std::fs::remove_file(d.join(format!("{id}.eml")));
    Ok(())
}

async fn skriv_atomisk(fil: &Path, data: &[u8]) -> Result<()> {
    let tmp = fil.with_extension("tmp");
    tokio::fs::write(&tmp, data).await.with_context(|| format!("kan ikke skrive {}", tmp.display()))?;
    tokio::fs::rename(&tmp, fil).await.with_context(|| format!("kan ikke omdøbe til {}", fil.display()))?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn proev_igen_flytter_tilbage() {
        let d = std::env::temp_dir().join(format!("smtpgraph-koe-{}", uuid::Uuid::new_v4().simple()));
        let k = Koe::ny(&d).unwrap();
        let mut p = k.laeg("a@b", &["c@d.dk".into()], "1.2.3.4:5", b"Subject: x\r\n\r\nhej").await.unwrap();
        p.forsoeg = 9;
        p.naeste = Utc::now() + Duration::hours(1);
        k.flyt(&p, "fejlet").await.unwrap();
        assert!(k.klar().await.unwrap().is_empty());
        let l = liste(&d);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].0, Status::Fejlet);
        proev_igen(&d, Status::Fejlet, l[0].1.clone()).unwrap();
        let klar = k.klar().await.unwrap();
        assert_eq!(klar.len(), 1);
        assert_eq!(klar[0].forsoeg, 0);
        assert!(k.eml(&klar[0].id).exists());
        slet(&d, Status::Venter, &klar[0].id).unwrap();
        assert!(liste(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
