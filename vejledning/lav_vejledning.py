"""Bygger Vejledning-SMTP-Graph.pdf (kundevejledning til første opsætning).

Kør:  python vejledning\\lav_vejledning.py
Skærmbillederne i billeder\\ er taget med:
  smtp-graph-gui.exe --ingen-admin --demo --config C:\\SMTP-Graph\\smtp-graph.toml --fane <fane> --skaermbillede <fil>
"""

from pathlib import Path

from PIL import Image
from reportlab.lib import colors
from reportlab.lib.enums import TA_LEFT
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.platypus import (
    CondPageBreak,
    Image as RLImage,
    KeepTogether,
    PageBreak,
    Paragraph,
    SimpleDocTemplate,
    Spacer,
    Table,
    TableStyle,
)

HER = Path(__file__).resolve().parent
BILLEDER = HER / "billeder"
UD = HER.parent / "udgivelse" / "Vejledning-SMTP-Graph.pdf"
import re
VERSION = re.search(r'^version\s*=\s*"([^"]+)"', (HER.parent / "Cargo.toml").read_text(encoding="utf-8"), re.M).group(1)
SETUP = f"SMTP-Graph-Setup-{VERSION}.exe"
MAPPE_TEKST = r"C:\SMTP-Graph"

FONTS = Path(r"C:\Windows\Fonts")
pdfmetrics.registerFont(TTFont("Segoe", str(FONTS / "segoeui.ttf")))
pdfmetrics.registerFont(TTFont("Segoe-Bold", str(FONTS / "segoeuib.ttf")))
pdfmetrics.registerFont(TTFont("Segoe-Italic", str(FONTS / "segoeuii.ttf")))
pdfmetrics.registerFont(TTFont("Segoe-Light", str(FONTS / "segoeuil.ttf")))
pdfmetrics.registerFont(TTFont("Consolas", str(FONTS / "consola.ttf")))
pdfmetrics.registerFontFamily("Segoe", normal="Segoe", bold="Segoe-Bold", italic="Segoe-Italic", boldItalic="Segoe-Bold")

BLAA = colors.HexColor("#1F4E79")
LYSBLAA = colors.HexColor("#EAF1F8")
GRAA = colors.HexColor("#5F6B76")
LINJE = colors.HexColor("#C9D3DD")
GUL = colors.HexColor("#FFF6DB")
GULKANT = colors.HexColor("#E8C766")

S = {
    "titel": ParagraphStyle("titel", fontName="Segoe-Bold", fontSize=26, leading=31, textColor=BLAA),
    "under": ParagraphStyle("under", fontName="Segoe-Light", fontSize=14, leading=19, textColor=GRAA),
    "h1": ParagraphStyle("h1", fontName="Segoe-Bold", fontSize=16, leading=21, textColor=BLAA, spaceBefore=10, spaceAfter=6),
    "h2": ParagraphStyle("h2", fontName="Segoe-Bold", fontSize=12, leading=16, textColor=BLAA, spaceBefore=8, spaceAfter=3),
    "t": ParagraphStyle("t", fontName="Segoe", fontSize=10, leading=14.5, alignment=TA_LEFT, spaceAfter=5),
    "lille": ParagraphStyle("lille", fontName="Segoe", fontSize=8.5, leading=11.5, textColor=GRAA),
    "celle": ParagraphStyle("celle", fontName="Segoe", fontSize=9.5, leading=13),
    "cellefed": ParagraphStyle("cellefed", fontName="Segoe-Bold", fontSize=9.5, leading=13),
    "punkt": ParagraphStyle("punkt", fontName="Segoe", fontSize=10, leading=14.5, leftIndent=14, bulletIndent=2, spaceAfter=2),
    "trin": ParagraphStyle("trin", fontName="Segoe", fontSize=10, leading=14.5, leftIndent=18, bulletIndent=0, spaceAfter=3),
    "billedtekst": ParagraphStyle("billedtekst", fontName="Segoe-Italic", fontSize=8.5, leading=11, textColor=GRAA, spaceBefore=3, spaceAfter=8),
}

BREDDE = A4[0] - 2 * 20 * mm


def p(tekst, stil="t"):
    return Paragraph(tekst, S[stil])


def kode(tekst):
    return f'<font name="Consolas" size="9.5">{tekst}</font>'


def knap(tekst):
    """En knap eller et felt i programmet."""
    return f"<b>{tekst}</b>"


def punkter(linjer):
    return [Paragraph(l, S["punkt"], bulletText="•") for l in linjer]


def trin(linjer):
    return [Paragraph(l, S["trin"], bulletText=f"{i}.") for i, l in enumerate(linjer, 1)]


def boks(indhold, baggrund=LYSBLAA, kant=LINJE):
    t = Table([[indhold]], colWidths=[BREDDE])
    t.setStyle(
        TableStyle(
            [
                ("BACKGROUND", (0, 0), (-1, -1), baggrund),
                ("BOX", (0, 0), (-1, -1), 0.8, kant),
                ("LEFTPADDING", (0, 0), (-1, -1), 10),
                ("RIGHTPADDING", (0, 0), (-1, -1), 10),
                ("TOPPADDING", (0, 0), (-1, -1), 7),
                ("BOTTOMPADDING", (0, 0), (-1, -1), 7),
            ]
        )
    )
    return t


def tip(titel, tekst):
    return boks([p(f"<b>{titel}</b>", "celle"), p(tekst, "celle")], GUL, GULKANT)


def tabel(raekker, bredder, hoved=True):
    data = []
    for i, r in enumerate(raekker):
        stil = "cellefed" if (hoved and i == 0) else "celle"
        data.append([p(c, stil) if isinstance(c, str) else c for c in r])
    t = Table(data, colWidths=bredder, repeatRows=1 if hoved else 0)
    stil = [
        ("VALIGN", (0, 0), (-1, -1), "TOP"),
        ("GRID", (0, 0), (-1, -1), 0.5, LINJE),
        ("LEFTPADDING", (0, 0), (-1, -1), 6),
        ("RIGHTPADDING", (0, 0), (-1, -1), 6),
        ("TOPPADDING", (0, 0), (-1, -1), 4),
        ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
    ]
    if hoved:
        stil.append(("BACKGROUND", (0, 0), (-1, 0), LYSBLAA))
    t.setStyle(TableStyle(stil))
    return t


def billede(navn, tekst, beskaer=None, skala=1.0):
    """Skærmbillede med tynd ramme. beskaer = (bredde, højde) i pixels fra øverste venstre hjørne.
    skala 1.0 = hele vinduet (1120 px) fylder sidens bredde."""
    kilde = BILLEDER / f"{navn}.png"
    img = Image.open(kilde)
    fuld = img.width
    if beskaer:
        img = img.crop((0, 0, min(beskaer[0], img.width), min(beskaer[1], img.height)))
        kilde = BILLEDER / f"_{navn}_beskaaret.png"
        img.save(kilde)
    bredde = min(BREDDE, BREDDE * img.width / fuld * skala)
    h = bredde * img.height / img.width
    rl = RLImage(str(kilde), width=bredde, height=h)
    ramme = Table([[rl]], colWidths=[bredde])
    ramme.setStyle(
        TableStyle(
            [
                ("BOX", (0, 0), (-1, -1), 0.8, LINJE),
                ("LEFTPADDING", (0, 0), (-1, -1), 0),
                ("RIGHTPADDING", (0, 0), (-1, -1), 0),
                ("TOPPADDING", (0, 0), (-1, -1), 0),
                ("BOTTOMPADDING", (0, 0), (-1, -1), 0),
            ]
        )
    )
    return KeepTogether([ramme, p(tekst, "billedtekst")])


def flow():
    """Scanner → SMTP-Graph → Microsoft 365 → modtager."""
    felter = [
        ("Scanner", "sender som almindelig\nSMTP på jeres net"),
        ("SMTP-Graph", "på en pc eller server,\nder altid er tændt"),
        ("Microsoft 365", "sender fra jeres\nscanner-postkasse"),
        ("Modtager", "får mailen som\nalmindelig e-mail"),
    ]
    celler, bredder = [], []
    for i, (navn, tekst) in enumerate(felter):
        celler.append([p(f"<b>{navn}</b>", "cellefed"), p(tekst.replace("\n", "<br/>"), "lille")])
        bredder.append(36 * mm)
        if i < len(felter) - 1:
            celler.append(p('<font size="16" color="#1F4E79">→</font>', "celle"))
            bredder.append(8 * mm)
    t = Table([celler], colWidths=bredder)
    stil = [("VALIGN", (0, 0), (-1, -1), "MIDDLE"), ("ALIGN", (0, 0), (-1, -1), "CENTER")]
    for i in range(0, len(celler), 2):
        stil += [
            ("BACKGROUND", (i, 0), (i, 0), LYSBLAA),
            ("BOX", (i, 0), (i, 0), 0.8, LINJE),
            ("TOPPADDING", (i, 0), (i, 0), 7),
            ("BOTTOMPADDING", (i, 0), (i, 0), 8),
        ]
    t.setStyle(TableStyle(stil))
    return t


def sidehoved_fod(c, doc):
    c.saveState()
    w, h = A4
    c.setStrokeColor(LINJE)
    c.setLineWidth(0.6)
    c.line(20 * mm, 14 * mm, w - 20 * mm, 14 * mm)
    c.setFont("Segoe", 8)
    c.setFillColor(GRAA)
    c.drawString(20 * mm, 9.5 * mm, f"SMTP-Graph {VERSION} · Kom i gang · Irvold")
    c.drawRightString(w - 20 * mm, 9.5 * mm, f"Side {doc.page}")
    c.restoreState()


def indhold():
    MAPPE = kode(MAPPE_TEKST)
    h = []
    # ------------------------------------------------------------------ forside
    h += [
        Spacer(1, 6 * mm),
        p("SMTP-Graph", "titel"),
        p("Scan til e-mail gennem Microsoft 365 – vejledning til første opsætning", "under"),
        Spacer(1, 8 * mm),
        p("Hvad gør SMTP-Graph?", "h1"),
        p(
            "Microsoft lukker for, at scannere og printere sender mail med brugernavn og adgangskode (SMTP-login). "
            "Mange scannere kan ikke andet, og så holder <i>scan til e-mail</i> op med at virke."
        ),
        p(
            "SMTP-Graph løser det. Programmet kører på en pc eller server på jeres eget net. Scanneren afleverer "
            "mailen til SMTP-Graph, præcis som den før gjorde til en mailserver, og SMTP-Graph sender den videre "
            "gennem Microsoft 365 fra én fast postkasse, fx <i>scanner@firma.dk</i>."
        ),
        Spacer(1, 3 * mm),
        flow(),
        Spacer(1, 5 * mm),
        p("Det får I", "h2"),
        *punkter(
            [
                "Scanneren skal ikke kende nogen adgangskode til Microsoft 365.",
                "Adgangen hos Microsoft er låst til den ene scanner-postkasse. Programmet kan ikke læse eller sende fra andre postkasser.",
                "Kun scanneren (eller de adresser, I vælger) må aflevere mail til SMTP-Graph.",
                "Mails, der ikke kan sendes med det samme, lægges i kø og sendes, så snart Microsoft svarer igen.",
                "Store scanninger (op til 35 MB) og ældre scannere virker også.",
            ]
        ),
        p("Før I går i gang", "h1"),
        tabel(
            [
                ["Det skal I bruge", "Bemærk"],
                ["En Windows-pc eller server, der altid er tændt", "På samme net som scanneren. Giv den gerne en fast IP-adresse."],
                ["Administratoradgang til pc'en", "Programmet beder selv om lov, når det starter."],
                [
                    "Login som administrator i Microsoft 365",
                    "Global administrator – eller både program- og Exchange-administrator. "
                    "Har I ikke det, se <i>Uden administrator</i> under trin 2.",
                ],
                [
                    "Den postkasse, der skal sende",
                    "Fx scanner@firma.dk. Findes den ikke, kan programmet oprette den som delt postkasse (kræver ingen licens).",
                ],
                ["Scannerens IP-adresse", "Står typisk i scannerens netværksmenu eller på en konfigurationsside, den kan udskrive."],
                ["Installationsprogrammet fra Irvold", f"{kode(SETUP)}"],
            ],
            [62 * mm, BREDDE - 62 * mm],
        ),
        PageBreak(),
    ]

    # ------------------------------------------------------------------ trin 1
    h += [
        p("Trin 1 · Installér programmet", "h1"),
        *trin(
            [
                f"Dobbeltklik på {kode(SETUP)}. Windows spørger, om programmet må foretage ændringer – svar {knap('Ja')}.",
                f"Følg guiden. Programmet lægges i {MAPPE}, og der kommer en genvej <i>SMTP-Graph</i> i Start-menuen og på skrivebordet.",
                f"Lad fluebenet ved {knap('Start SMTP-Graph nu')} være sat, og klik på {knap('Færdig')}.",
                f"Første gang er der ingen indstillinger. Klik på {knap('Opret indstillinger')}.",
            ]
        ),
        tip(
            "Hvis Windows siger »Windows beskyttede din pc«",
            f"Klik på {knap('Flere oplysninger')} og derefter {knap('Kør alligevel')}. Advarslen kommer, fordi "
            "installationsprogrammet er nyt og endnu ikke kendt af Microsoft Defender SmartScreen.",
        ),
        p(
            f"Programmet har fem faner: {knap('Oversigt')}, {knap('Microsoft 365')}, {knap('Modtager (SMTP)')}, "
            f"{knap('Kø')} og {knap('Log')}. Øverst ses tjenestens status og knapperne til at styre den."
        ),
        # ------------------------------------------------------------------ trin 2
        p("Trin 2 · Forbind til Microsoft 365", "h1"),
        p(f"Gå til fanen {knap('Microsoft 365')} og vælg {knap('Administrator opretter app (anbefalet)')}."),
        *trin(
            [
                f"{knap('Postkasse')}: den adresse, scanningerne skal komme fra, fx scanner@firma.dk.",
                f"{knap('Stedets navn')}: et navn, I kan genkende, fx »Firma ApS kontor«. Det bruges i navnet på appen i Microsoft 365.",
                f"Sæt flueben i {knap('Opret postkassen som delt postkasse, hvis den ikke findes')}, hvis postkassen ikke findes endnu.",
                f"Klik på {knap('Log ind som administrator og sæt op')}. Jeres browser åbner Microsofts login.",
                "Log ind med administratorkontoen, og godkend de tilladelser, Microsoft viser for <i>SMTP-Graph (Irvold)</i>.",
                "Gå tilbage til programmet. Det opretter nu det hele og skriver undervejs, hvad det gør. Det tager typisk 1–3 minutter.",
            ]
        ),
        billede("ms", "Fanen Microsoft 365. Når opsætningen er færdig, står den nye afsender under »Nu«.", beskaer=(1110, 580)),
        tip(
            "Microsoft skriver måske »ikke bekræftet« ved udgiveren",
            "Det er Irvolds login-app, som kun bruges, mens opsætningen kører. Bagefter bruger SMTP-Graph jeres egen app.",
        ),
        Spacer(1, 4 * mm),
        p("Hvad bliver oprettet i Microsoft 365?", "h2"),
        *punkter(
            [
                "En app i jeres Microsoft Entra ID med navnet <i>SMTP-Graph – stedets navn</i>.",
                "En regel i Exchange, så appen kun må sende fra den valgte postkasse (og kun læse/skrive i den, når store vedhæftninger skal uploades).",
                "En adgangsnøgle, der gælder i 2 år. Den gemmes kun på pc'en, i en fil som kun administratorer kan læse.",
                "Postkassen som delt postkasse, hvis I bad om det.",
            ]
        ),
        p("Uden administrator", "h2"),
        p(
            f"Har I ikke en administrator ved hånden, kan I i stedet vælge {knap('Log ind som postkassen')} og "
            f"klikke på {knap('Log ind med Microsoft')}. Log ind som den konto, der skal sende (den skal have en licens). "
            "Programmet holder selv login i live, men skifter kontoen adgangskode, skal I logge ind igen her."
        ),
        # ------------------------------------------------------------------ trin 3
        p("Trin 3 · Send en testmail", "h1"),
        p(
            f"Under {knap('Afprøv')} på samme fane: skriv jeres egen adresse i {knap('Testmail til')}, og klik på {knap('Send')}. "
            "Mailen skal komme frem fra scanner-postkassen. Vil I også prøve store vedhæftninger, så sæt flueben i "
            f"{knap('med 5 MB vedhæftning')}."
        ),
        p(
            "Kommer der en fejl om manglende rettigheder, så vent et par minutter og prøv igen – nye rettigheder i "
            "Exchange kan være lidt om at slå igennem (i sjældne tilfælde op til 2 timer).",
            "lille",
        ),
    ]

    # ------------------------------------------------------------------ trin 4
    h += [
        CondPageBreak(60 * mm),
        p("Trin 4 · Hvem må aflevere mail?", "h1"),
        p(f"Gå til fanen {knap('Modtager (SMTP)')}. De fleste felter kan blive, som de er. Det vigtige er:"),
        tabel(
            [
                ["Felt", "Hvad I skriver"],
                [knap("Må forbinde fra"), "Scannerens IP-adresse, fx 192.168.1.50. Én pr. linje – eller et helt net som 192.168.1.0/24."],
                [knap("Lyt på"), f"Lad stå: {kode('0.0.0.0:25')} (port 25). Vil scanneren have port 587, så tilføj {kode('0.0.0.0:587')}."],
                [knap("Tilladte modtagere"), "Anbefalet: jeres eget domæne, fx firma.dk. Så kan scanneren ikke bruges til at sende ud af huset. Tom = alle."],
                [knap("Besked ved fejl til"), "En adresse, der får besked, hvis en mail ikke kan sendes, fx it@firma.dk."],
                [knap("Afsendernavn"), "Fx »Scanner«. Exchange viser dog ofte postkassens eget navn."],
            ],
            [42 * mm, BREDDE - 42 * mm],
        ),
        Spacer(1, 2 * mm),
        p(f"Klik på {knap('Gem')} nederst på siden."),
        billede("modtager", "Fanen Modtager (SMTP), øverste del. Her må kun scanneren 192.168.1.50 aflevere mail.", beskaer=(690, 448), skala=1.1),
        # ------------------------------------------------------------------ trin 5
        p("Trin 5 · Start tjenesten", "h1"),
        p(
            f"Klik på {knap('Installér tjeneste')} øverst. SMTP-Graph bliver nu en Windows-tjeneste, der starter af sig selv, "
            "også når ingen er logget ind på pc'en. Samtidig åbnes Windows Firewall – kun for de adresser, I skrev under "
            f"{knap('Må forbinde fra')}."
        ),
        p(f"Når tjenesten kører, står der {knap('Tjeneste: Kører')} i grønt øverst."),
        tip(
            "Har I ændret noget?",
            f"Efter ændringer i Modtager-fanen: klik {knap('Genstart')}. Har I ændret porte eller IP-adresser, så klik "
            f"{knap('Geninstallér')}, så firewallen også følger med.",
        ),
    ]

    # ------------------------------------------------------------------ trin 6
    h += [
        CondPageBreak(70 * mm),
        p("Trin 6 · Indstil scanneren", "h1"),
        p(
            f"Fanen {knap('Oversigt')} viser præcis, hvad scanneren skal have, under {knap('Sådan indstilles scanneren')}. "
            "Find scannerens indstillinger for <i>scan til e-mail</i> eller <i>udgående e-mail / SMTP</i>, og udfyld:"
        ),
        tabel(
            [
                ["Indstilling på scanneren", "Værdi"],
                ["SMTP-server", "Pc'ens IP-adresse (står øverst på Oversigt, fx 192.168.1.20)"],
                ["Port", "25"],
                [
                    "Sikker forbindelse (SSL/TLS, STARTTLS)",
                    "Valgfrit – begge dele virker. Også ældre scannere, der kun kan gammel kryptering.",
                ],
                ["Valider / bekræft servercertifikat", "<b>Fra</b> (pc'en bruger sit eget certifikat, som scanneren ikke kender)"],
                ["Godkendelse / login", "<b>Ingen</b> – adgangen styres af scannerens IP-adresse"],
                ["Afsender / fra-adresse", "Hvad som helst, fx scanner@firma.dk. Mailen sendes altid fra scanner-postkassen."],
            ],
            [62 * mm, BREDDE - 62 * mm],
        ),
        Spacer(1, 3 * mm),
        p("Gem på scanneren, og send en testscanning til jer selv."),
        billede("oversigt", "Fanen Oversigt: det, scanneren skal have, og forbindelsen til Microsoft 365.", beskaer=(700, 548), skala=1.1),
    ]

    # ------------------------------------------------------------------ trin 7
    h += [
        CondPageBreak(60 * mm),
        p("Trin 7 · Kø og log", "h1"),
        p(
            f"{knap('Kø')} viser mails, der venter. Kan en mail ikke sendes, prøver SMTP-Graph igen efter 1, 2, 5, 10 og 30 minutter "
            f"og derefter hver time i op til 48 timer. Lykkes det ikke, flyttes mailen til {knap('Opgivet')}, og adressen under "
            f"{knap('Besked ved fejl til')} får en mail om det. I køen kan I {knap('Send nu')}, {knap('Åbn')} eller {knap('Slet')} en mail."
        ),
        p(
            f"{knap('Log')} viser alt, hvad tjenesten har gjort i dag. Hver scanning giver en linje med »modtog« og en med »sendt til«. "
            f"Sæt flueben i {knap('Kun advarsler og fejl')} for at finde problemerne hurtigt."
        ),
        billede("log", "Fanen Log: to scanninger modtaget fra scanneren og sendt videre gennem Microsoft 365.", beskaer=(1110, 345)),
        p("Ny version og afinstallation", "h2"),
        p(
            "Kør blot det nye installationsprogram. Det finder selv mappen, hvor SMTP-Graph kører, stopper tjenesten et øjeblik "
            "og starter den igen bagefter. Indstillinger, kø, log og forbindelsen til Microsoft 365 bliver liggende."
        ),
        p(
            f"SMTP-Graph fjernes under {knap('Indstillinger')} → {knap('Apps')} → {knap('Installerede apps')}. Tjenesten og "
            "firewall-reglen fjernes, og I bliver spurgt, om indstillinger, kø og log også skal slettes."
        ),
    ]

    # ------------------------------------------------------------------ fejlfinding
    h += [
        CondPageBreak(60 * mm),
        p("Hvis det ikke virker", "h1"),
        tabel(
            [
                ["Det sker", "Gør sådan"],
                [
                    "Scanneren kan slet ikke forbinde",
                    "Står der <b>Kører</b> øverst i programmet? Er scannerens IP skrevet under <b>Må forbinde fra</b>? "
                    "Har I ændret IP eller port, så klik <b>Geninstallér</b> (firewallen).",
                ],
                [
                    "Scanneren melder fejl om sikker forbindelse eller certifikat",
                    "Slå <b>Valider servercertifikat</b> fra på scanneren. Hjælper det ikke, så slå sikker forbindelse helt fra – "
                    "trafikken bliver på jeres eget net.",
                ],
                [
                    "Scanneren melder »protokol understøttes ikke« eller lignende",
                    "Se i <b>Log</b>. En linje med »ingen mail i forbindelsen« viser, hvor langt scanneren kom. Send linjen til Irvold.",
                ],
                [
                    "Mailen står i Kø med en fejl",
                    "Læs fejlen i køen. Klik <b>Prøv login hos Microsoft</b> på fanen Microsoft 365 for at se, om forbindelsen virker.",
                ],
                [
                    "Store scanninger (over ca. 3 MB) kommer ikke frem",
                    "Opsætningen er lavet med <b>Kun Mail.Send</b>. Kør trin 2 igen uden det flueben.",
                ],
                [
                    "»Login udløbet – log ind igen«",
                    "Kun ved <i>Log ind som postkassen</i>: kontoen har skiftet adgangskode. Log ind igen på fanen Microsoft 365.",
                ],
                [
                    "Adgangsnøglen udløber snart",
                    "Datoen står på Oversigt. Kør trin 2 igen med samme postkasse – så får appen en ny nøgle i 2 år.",
                ],
            ],
            [58 * mm, BREDDE - 58 * mm],
        ),
        Spacer(1, 6 * mm),
        boks(
            [
                p("<b>Brug for hjælp?</b>", "celle"),
                p(
                    "Kontakt Irvold på support@irvold.dk. Send gerne et skærmbillede af fanen Oversigt og de seneste linjer fra Log. "
                    "Loggen indeholder ingen adgangskoder.",
                    "celle",
                ),
            ]
        ),
    ]
    return h


def main():
    UD.parent.mkdir(exist_ok=True)
    doc = SimpleDocTemplate(
        str(UD),
        pagesize=A4,
        leftMargin=20 * mm,
        rightMargin=20 * mm,
        topMargin=18 * mm,
        bottomMargin=20 * mm,
        title="SMTP-Graph – Kom i gang",
        author="Irvold",
        subject="Vejledning til første opsætning af SMTP-Graph",
    )
    doc.build(indhold(), onFirstPage=sidehoved_fod, onLaterPages=sidehoved_fod)
    for f in BILLEDER.glob("_*_beskaaret.png"):
        f.unlink()
    print(UD)


if __name__ == "__main__":
    main()
