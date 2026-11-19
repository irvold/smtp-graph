//! Tilretning af mailen, før den sendes gennem Graph.
//!
//! Lille mail: den rå MIME sendes direkte (kræver kun Mail.Send). Her skrives From om til den
//! låste afsender, og To/Cc/Bcc bygges, så kun konvolutmodtagerne (RCPT TO) får den.
//! Stor mail: den pakkes ud til Graph-JSON + vedhæftninger (kræver Mail.ReadWrite).

use base64::Engine;
use mail_parser::{MessageParser, MimeHeaders};
use serde_json::{json, Value};
use std::collections::HashSet;

pub struct Adr {
    pub adresse: String,
    pub navn: Option<String>,
}

/// Modtagerne fordelt på To/Cc/Bcc, kun blandt konvolutmodtagerne.
pub struct Fordeling {
    pub to: Vec<Adr>,
    pub cc: Vec<Adr>,
    pub bcc: Vec<Adr>,
    /// Var header-felterne uændrede (så de kan bevares ordret)?
    pub to_uaendret: bool,
    pub cc_uaendret: bool,
}

fn adresser(a: Option<&mail_parser::Address>) -> Vec<Adr> {
    a.map(|a| {
        a.iter()
            .filter_map(|x| {
                Some(Adr { adresse: x.address()?.trim().to_string(), navn: x.name().map(str::to_string) })
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn fordel(msg: Option<&mail_parser::Message>, konvolut: &[String]) -> Fordeling {
    let env: HashSet<String> = konvolut.iter().map(|a| a.to_ascii_lowercase()).collect();
    let hto = adresser(msg.and_then(|m| m.to()));
    let hcc = adresser(msg.and_then(|m| m.cc()));
    let n_to = hto.len();
    let n_cc = hcc.len();
    let mut brugt = HashSet::new();
    let mut filtrer = |v: Vec<Adr>| -> Vec<Adr> {
        v.into_iter()
            .filter(|a| {
                let l = a.adresse.to_ascii_lowercase();
                env.contains(&l) && brugt.insert(l)
            })
            .collect()
    };
    let to = filtrer(hto);
    let cc = filtrer(hcc);
    let rest: Vec<Adr> = konvolut
        .iter()
        .filter(|a| !brugt.contains(&a.to_ascii_lowercase()))
        .map(|a| Adr { adresse: a.clone(), navn: None })
        .collect();
    if to.is_empty() && cc.is_empty() {
        return Fordeling { to: rest, cc: vec![], bcc: vec![], to_uaendret: false, cc_uaendret: n_cc == 0 };
    }
    Fordeling { to_uaendret: to.len() == n_to, cc_uaendret: cc.len() == n_cc, to, cc, bcc: rest }
}

pub fn parse(raw: &[u8]) -> Option<mail_parser::Message<'_>> {
    MessageParser::default().parse(raw)
}

/// Original afsender (From-header, ellers konvolutten).
fn original_fra(msg: Option<&mail_parser::Message>, env_fra: &str) -> Option<String> {
    msg.and_then(|m| m.from())
        .and_then(|f| f.first())
        .and_then(|a| a.address())
        .map(str::to_string)
        .or_else(|| (!env_fra.is_empty()).then(|| env_fra.to_string()))
}

pub struct Afsender<'a> {
    pub from: &'a str,
    pub navn: Option<&'a str>,
    pub svar_til_original: bool,
}

fn header_ord(s: &str) -> String {
    if s.is_ascii() {
        format!("\"{}\"", s.replace(['"', '\\'], ""))
    } else {
        format!("=?UTF-8?B?{}?=", base64::engine::general_purpose::STANDARD.encode(s))
    }
}

fn adr_liste(v: &[Adr]) -> String {
    v.iter().map(|a| format!("<{}>", a.adresse)).collect::<Vec<_>>().join(",\r\n ")
}

/// Omskriver headerne i den rå mail til Graphs MIME-afsendelse.
pub fn omskriv(raw: &[u8], env_fra: &str, konvolut: &[String], afs: &Afsender) -> Vec<u8> {
    let msg = parse(raw);
    let ford = fordel(msg.as_ref(), konvolut);
    let orig = original_fra(msg.as_ref(), env_fra);
    let har_reply_to = msg.as_ref().is_some_and(|m| m.reply_to().is_some());
    drop(msg);

    let (hoved, krop) = match find(raw, b"\r\n\r\n") {
        Some(i) => (&raw[..i + 2], &raw[i + 2..]),
        None => (raw, &b"\r\n"[..]),
    };
    // Del headerne op i felter (med foldede fortsættelseslinjer).
    let mut felter: Vec<(String, &[u8])> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < hoved.len() {
        let slut = find(&hoved[i..], b"\r\n").map(|j| i + j + 2).unwrap_or(hoved.len());
        let naeste_fortsaetter = hoved.get(slut).is_some_and(|c| *c == b' ' || *c == b'\t');
        if !naeste_fortsaetter {
            let felt = &hoved[start..slut];
            let navn = felt.split(|c| *c == b':').next().unwrap_or_default();
            felter.push((String::from_utf8_lossy(navn).trim().to_ascii_lowercase(), felt));
            start = slut;
        }
        i = slut;
    }

    let mut ud = Vec::with_capacity(raw.len() + 512);
    let fra = match afs.navn {
        Some(n) if !n.is_empty() => format!("From: {} <{}>\r\n", header_ord(n), afs.from),
        _ => format!("From: <{}>\r\n", afs.from),
    };
    ud.extend_from_slice(fra.as_bytes());
    let svar = afs.svar_til_original
        && !har_reply_to
        && orig.as_deref().is_some_and(|o| !o.eq_ignore_ascii_case(afs.from));
    if svar {
        ud.extend_from_slice(format!("Reply-To: <{}>\r\n", orig.unwrap()).as_bytes());
    }
    for (navn, felt) in &felter {
        match navn.as_str() {
            "from" | "sender" | "return-path" | "bcc" | "x-sender" => {}
            "to" if ford.to_uaendret => ud.extend_from_slice(felt),
            "cc" if ford.cc_uaendret => ud.extend_from_slice(felt),
            "to" | "cc" => {}
            _ => ud.extend_from_slice(felt),
        }
    }
    let har = |n: &str| felter.iter().any(|(f, _)| f == n);
    if !ford.to.is_empty() && !(ford.to_uaendret && har("to")) {
        ud.extend_from_slice(format!("To: {}\r\n", adr_liste(&ford.to)).as_bytes());
    }
    if !ford.cc.is_empty() && !(ford.cc_uaendret && har("cc")) {
        ud.extend_from_slice(format!("Cc: {}\r\n", adr_liste(&ford.cc)).as_bytes());
    }
    if !ford.bcc.is_empty() {
        ud.extend_from_slice(format!("Bcc: {}\r\n", adr_liste(&ford.bcc)).as_bytes());
    }
    ud.extend_from_slice(b"\r\n");
    ud.extend_from_slice(krop.strip_prefix(b"\r\n").unwrap_or(krop));
    ud
}

pub struct Vedhaeftning {
    pub navn: String,
    pub type_: String,
    pub data: Vec<u8>,
    pub content_id: Option<String>,
    pub inline: bool,
}

/// Pakker en mail ud til Graphs JSON-format (til store mails).
pub fn til_json(raw: &[u8], env_fra: &str, konvolut: &[String], afs: &Afsender) -> (Value, Vec<Vedhaeftning>) {
    let msg = parse(raw);
    let m = msg.as_ref();
    let ford = fordel(m, konvolut);
    let modt = |v: &[Adr]| -> Value {
        v.iter()
            .map(|a| match &a.navn {
                Some(n) => json!({"emailAddress": {"address": a.adresse, "name": n}}),
                None => json!({"emailAddress": {"address": a.adresse}}),
            })
            .collect()
    };
    // mail-parser laver selv HTML af ren tekst; brug kun HTML, hvis mailen faktisk har en HTML-del.
    let ægte_html = m.is_some_and(|m| {
        m.html_bodies().any(|d| d.content_type().and_then(|c| c.subtype()).is_some_and(|s| s.eq_ignore_ascii_case("html")))
    });
    let (type_, indhold) = match m.and_then(|m| m.body_html(0)).filter(|_| ægte_html) {
        Some(h) => ("HTML", h.into_owned()),
        None => ("Text", m.and_then(|m| m.body_text(0)).map(|t| t.into_owned()).unwrap_or_default()),
    };
    let mut from = json!({"emailAddress": {"address": afs.from}});
    if let Some(n) = afs.navn.filter(|n| !n.is_empty()) {
        from["emailAddress"]["name"] = json!(n);
    }
    let mut besked = json!({
        "subject": m.and_then(|m| m.subject()).unwrap_or(""),
        "body": {"contentType": type_, "content": indhold},
        "toRecipients": modt(&ford.to),
        "ccRecipients": modt(&ford.cc),
        "bccRecipients": modt(&ford.bcc),
        "from": from,
    });
    let orig_svar = adresser(m.and_then(|m| m.reply_to()));
    if !orig_svar.is_empty() {
        besked["replyTo"] = modt(&orig_svar);
    } else if afs.svar_til_original {
        if let Some(o) = original_fra(m, env_fra).filter(|o| !o.eq_ignore_ascii_case(afs.from)) {
            besked["replyTo"] = json!([{"emailAddress": {"address": o}}]);
        }
    }

    let mut vedh = Vec::new();
    if let Some(m) = m {
        for (n, del) in m.attachments().enumerate() {
            let ct = del.content_type();
            let type_ = ct
                .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
                .unwrap_or_else(|| "application/octet-stream".into());
            let navn = del.attachment_name().map(str::to_string).unwrap_or_else(|| {
                let ext = ct.and_then(|c| c.subtype()).unwrap_or("bin");
                format!("vedhaeftning-{}.{ext}", n + 1)
            });
            let cid = del.content_id().map(|c| c.trim_matches(['<', '>']).to_string());
            let inline = cid.is_some() && del.content_disposition().is_none_or(|d| d.is_inline());
            vedh.push(Vedhaeftning { navn, type_, data: del.contents().to_vec(), content_id: cid, inline });
        }
    }
    (besked, vedh)
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

#[cfg(test)]
mod test {
    use super::*;

    const MAIL: &[u8] = b"From: Scanner <scan@lokal>\r\nTo: Anna <anna@firma.dk>, x@ude.dk\r\nSubject: Scan\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nHej\r\n";

    fn afs() -> Afsender<'static> {
        Afsender { from: "scanner@firma.dk", navn: None, svar_til_original: true }
    }

    #[test]
    fn omskriver_from_og_modtagere() {
        let ud = omskriv(MAIL, "scan@lokal", &["anna@firma.dk".into(), "bo@firma.dk".into()], &afs());
        let s = String::from_utf8(ud).unwrap();
        assert!(s.starts_with("From: <scanner@firma.dk>\r\nReply-To: <scan@lokal>\r\n"), "{s}");
        assert!(s.contains("To: <anna@firma.dk>\r\n"), "{s}");
        assert!(!s.contains("x@ude.dk"), "{s}");
        assert!(s.contains("Bcc: <bo@firma.dk>\r\n"), "{s}");
        assert!(s.ends_with("\r\n\r\nHej\r\n"), "{s}");
    }

    #[test]
    fn bevarer_uaendret_to() {
        let ud = omskriv(MAIL, "", &["anna@firma.dk".into(), "x@ude.dk".into()], &afs());
        let s = String::from_utf8(ud).unwrap();
        assert!(s.contains("To: Anna <anna@firma.dk>, x@ude.dk\r\n"), "{s}");
        assert!(!s.contains("Bcc"), "{s}");
    }

    #[test]
    fn uden_to_header() {
        let m = b"Subject: x\r\n\r\nkrop";
        let s = String::from_utf8(omskriv(m, "", &["a@b.dk".into()], &afs())).unwrap();
        assert!(s.contains("To: <a@b.dk>\r\n"), "{s}");
        assert!(s.ends_with("\r\n\r\nkrop"), "{s}");
    }
}
