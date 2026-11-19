# SMTP-Graph

Tager imod mail over SMTP (fx fra en scanner eller printer) og sender dem videre gennem
Microsoft Graph. Brug det, når Microsoft 365 ikke længere tager imod SMTP med adgangskode.

```
Scanner ──SMTP──> smtp-graph.exe ──HTTPS (Graph)──> Microsoft 365 ──> modtageren
                   │
                   └─ koe\  (mails venter her, hvis Microsoft ikke svarer)
```

## Det kan den

- SMTP på port 25/587 med STARTTLS og TLS fra start (465), også for ældre scannere via Windows' TLS. Selvsigneret certifikat laves selv.
- Login (AUTH PLAIN/LOGIN) er valgfrit. Uden login er adgangen styret af `tilladte_ip`.
- Mails over ca. 3 MB pakkes ud og sendes som kladde + upload i bidder (op til 150 MB pr. vedhæftning).
- Afsenderen bliver altid den låste postkasse. Scannerens egen adresse kan lægges i Reply-To.
- Kun konvolutmodtagerne (RCPT TO) får mailen; ekstra modtagere sendes som Bcc.
- Diskkø med genforsøg (1, 2, 5, 10, 30 min, derefter hver time i 48 timer). Opgivne mails lægges i
  `koe\fejlet`, og `fejl_til` får besked.
- `tilladte_modtagere` kan låse den til fx kun firmaets eget domæne.
- Kører som Windows-tjeneste med genstart ved fejl og daglig log i `log\`.

## Opsætning med programmet (GUI)

Kør `SMTP-Graph-Setup-<version>.exe` fra `udgivelse` (lav den med `byg.bat`). Den installerer i
`C:\SMTP-Graph` – eller i den mappe, tjenesten SMTPGraph allerede kører fra – med genvej i Start-menuen
og på skrivebordet, og starter `smtp-graph-gui.exe`, der beder om administratorrettigheder (UAC).
En ny version installeres oven i den gamle: tjenesten stoppes under kopieringen og startes igen, og
indstillinger, kø og log bliver liggende. Afinstallation (Indstillinger → Apps) fjerner tjenesten og
firewall-reglen og spørger, om indstillinger, kø og log også skal slettes.
Uden installer kan indholdet af `udgivelse` også bare kopieres til mappen.

| Fane | |
|---|---|
| **Oversigt** | Sådan indstilles scanneren: pc'ens IP-adresser, porte og kryptering (og om de lytter), login, hvem der må forbinde, Microsoft 365-status, hemmelighedens udløb, kø og seneste log |
| **Microsoft 365** | Vælg *Administrator opretter app* eller *Log ind som postkassen* (se herunder). Prøv login og send en testmail. Under Avanceret: fælles login-app, PowerShell-vejen og manuel kode |
| **Modtager (SMTP)** | Porte, TLS, tilladte IP, brugere, tilladte modtagere, afsendernavn, besked ved fejl, tørløb. Gem bevarer kommentarerne i filen |
| **Kø** | Ventende, opgivne og tørløbs-mails. Send nu, åbn .eml, slet |
| **Log** | Dagens log med søgning og »kun advarsler og fejl« |
| **Om** | Version og udvikler (Irvold IT & Kommunikation, 4750 Lundby, www.irvold.dk) |

Øverst: tjenestens tilstand og knapperne Start, Stop, Genstart, Installér og Afinstallér.
GUI'en bruger samme indstillingsfil som den installerede tjeneste.
Opsætningsscriptet er bygget ind i GUI'en og kræver PowerShell 7 (`winget install Microsoft.PowerShell`).

## Linux

Samme motor uden GUI, som systemd-tjeneste. Pakkerne `udgivelse\linux\smtp-graph-<version>-linux-x86_64.tar.gz`
og `…-aarch64.tar.gz` (Raspberry Pi 4/5 m.fl.) er statiske (musl) og kører på alle 64-bit-distributioner med systemd.
Trin for trin står i `linux/LAES-MIG.txt` (ligger i pakken):

```
sudo ./smtp-graph installer        # program → /usr/local/bin, data → /var/lib/smtp-graph, bruger smtp-graph
sudo nano /var/lib/smtp-graph/smtp-graph.toml
sudo smtp-graph opsaet --postkasse scanner@firma.dk --navn "Firma kontor" --opret-delt
sudo smtp-graph installer          # starter tjenesten, når Microsoft 365 er sat op
```

- Tjenesten kører som systembrugeren `smtp-graph` med kun `CAP_NET_BIND_SERVICE` og `ProtectSystem=strict`.
  Programmet ejes af root, så tjenesten ikke kan ændre det.
- Kommandoer med `sudo` (tjek, test, opsaet, login, koe) skifter selv til brugeren `smtp-graph`, så nye filer får den rigtige ejer.
- Login uden skærm: programmet viser login-adressen; log ind i en browser på en hvilken som helst pc, og indsæt
  den adresse, browseren ender på (`http://127.0.0.1:…/?code=…`), i terminalen.
- Tjenesten slås først til ved opstart (`systemctl enable`), når den kan starte, så den ikke genstarter i ring.
- ufw åbnes for portene fra `tilladte_ip`, hvis ufw er slået til. Andre firewalls må åbnes i hånden.
- Ingen Windows-TLS-reserve: ældre scannere, som rustls ikke kan tale med, må sende uden kryptering på det lokale net.
  HP PageWide 477 prøver STARTTLS, så snart det tilbydes, også med »sikker forbindelse« slået fra – sæt `starttls = false`.
- `afinstaller` fjerner tjenesten og ufw-reglerne; data og program bliver liggende (kommandoen skriver, hvordan de fjernes).

Byg (i WSL Ubuntu med cargo-zigbuild og zig i `~/.zig-venv`):
```powershell
wsl -d Ubuntu -- bash /mnt/c/Arbejdsmappen/SMTP-Graph-rs/linux/byg-linux.sh
```
Afprøvet i WSL Ubuntu 26.04 med systemd (installation, login hos Microsoft, STARTTLS, tørløb, SIGTERM, afinstallation).
aarch64-udgaven er bygget, men ikke kørt på rigtig ARM-hardware.

## Microsoft 365: to måder at logge ind på

Begge logger ind i browseren på **kundens** Microsoft 365 gennem Irvolds fælles login-app
(multi-tenant, uden hemmelighed). Den fælles app oprettes **én gang** i Irvolds egen Microsoft 365:

```powershell
pwsh C:\Arbejdsmappen\SMTP-Graph-rs\Opret-FaellesApp.ps1
```
App-id'et indsættes under Microsoft 365 → Avanceret (eller bygges ind i `oauth::FAELLES_APP`).

| | Administrator opretter app (anbefalet) | Log ind som postkassen |
|---|---|---|
| Hvem logger ind | Administrator hos kunden | Kontoen, der skal sende (kræver licens) |
| Hvad sker der | Programmet opretter kundens egen app, hemmelighed (24 mdr.) og låsning til postkassen i Exchange – som Opsaet-SmtpGraph.ps1, men uden PowerShell | Login-token gemmes i `graph-login.json` og fornyes af tjenesten hver dag |
| Sender gennem | Kundens egen app (client credentials) | Irvolds fælles app på brugerens vegne |
| Delt postkasse | Ja (anbefalet) | Kun via en bruger med »Send som« + fuld adgang |
| Skal gentages | Når hemmeligheden udløber (loggen advarer) | Ved skift af adgangskode, spærring eller 90 dage uden fornyelse |

Administrator-vejen bruger Exchange Onlines REST-indgang (`adminapi/beta/{tenant}/InvokeCommand`),
som Microsofts eget PowerShell-modul bruger. Den er ikke officielt dokumenteret; virker den ikke,
findes PowerShell-vejen stadig under Avanceret.

Kunder ser den fælles app som »ikke bekræftet udgiver«, indtil Irvold er bekræftet udgiver.
Administratorer kan altid godkende den; almindelige brugere kun, hvis kundens indstillinger tillader det.

## Opsætning fra kommandolinjen

1. Kopiér `smtp-graph.exe` og `Opsaet-SmtpGraph.ps1` til fx `C:\SMTP-Graph` på en pc/server, der altid er tændt.
2. Lav indstillingsfilen:
   ```powershell
   C:\SMTP-Graph\smtp-graph.exe ny-config
   ```
   Ret `tilladte_ip` til scannerens adresse (eller nettet).
3. Sæt Microsoft 365 op (PowerShell 7, log ind som administrator hos kunden):
   ```powershell
   pwsh C:\SMTP-Graph\Opsaet-SmtpGraph.ps1 -Postkasse scanner@kunde.dk -Navn "kunde.dk kontor" -OpretDelt -Config C:\SMTP-Graph\smtp-graph.toml
   ```
   Scriptet laver en app, der kun har adgang til `scanner@kunde.dk` (RBAC for Applications i
   Exchange: Mail.Send + Mail.ReadWrite), og skriver koden ind i indstillingsfilen.
   `-KunSend` giver kun Mail.Send (så er grænsen ca. 3 MB pr. mail).
4. Tjek og send en testmail:
   ```powershell
   C:\SMTP-Graph\smtp-graph.exe tjek
   ```
   ```powershell
   C:\SMTP-Graph\smtp-graph.exe test --til dig@kunde.dk --stor
   ```
   Nye rettigheder i Exchange kan tage op til 2 timer.
5. Installér tjenesten (som administrator):
   ```powershell
   C:\SMTP-Graph\smtp-graph.exe installer
   ```
   Det opretter tjenesten `SMTPGraph` (LocalSystem, automatisk start), åbner firewallen for portene
   fra `tilladte_ip` og begrænser adgangen til indstillingsfilen til SYSTEM og administratorer.
6. Indstil scanneren: SMTP-server = pc'ens IP, port 25, ingen kryptering/login (eller STARTTLS + login,
   hvis `[[smtp.bruger]]` er sat). Afsenderadressen er ligegyldig.

Tip: Prøv først med `toerlob = true`. Så gemmes mails i `koe\toerlob` i stedet for at blive sendt.

## Kommandoer

| Kommando | |
|---|---|
| `smtp-graph.exe` | Kør i konsollen (Ctrl+C stopper) |
| `ny-config` | Skriv en indstillingsfil med forklaringer |
| `tjek` | Vis opsætningen og prøv login hos Microsoft |
| `test --til <adr> [--stor]` | Send en testmail direkte (med `--stor` en 5 MB vedhæftning) |
| `koe` | Vis ventende og opgivne mails |
| `installer` / `afinstaller` | Windows-tjenesten |

`--config <fil>` vælger en anden indstillingsfil end `smtp-graph.toml` ved siden af programmet.

## Forny hemmeligheden

Hemmeligheden gælder 24 måneder. Loggen advarer 30 dage før.

```powershell
pwsh C:\SMTP-Graph\Opsaet-SmtpGraph.ps1 -Postkasse scanner@kunde.dk -Navn "kunde.dk kontor" -Forny -Config C:\SMTP-Graph\smtp-graph.toml
```
```powershell
Restart-Service SMTPGraph
```

## Godt at vide

- TLS: `tls_motor = "auto"` (standard) bruger rustls til moderne klienter. Tilbyder scanneren kun
  ældre kryptering (RSA/DHE-nøgleudveksling, ingen fælles kurver – rustls melder fx
  `NoKxGroupsInCommon`, scanneren "ingen sikker forbindelse"), tager Windows' egen TLS (Schannel)
  over på samme forbindelse. Loggen skriver "ældre TLS, Windows tager over" med det, klienten
  tilbød. Det selvsignerede certifikat er RSA 2048 (`cert-rsa.pem`), så begge kan bruge det.
- Afprøvet med HP PageWide 477 ("Brug altid sikker forbindelse", port 25, ingen godkendelse):
  den prøver først TLS direkte på port 25 (giver "500" i loggen), derefter STARTTLS via Windows.
  HP'en går i stå efter `354`, hvis et svar deles over flere TLS-pakker, så hvert svar sendes samlet.
- TLS 1.0/1.1 er slået fra i nyere Windows 11. En scanner, der kun kan det, må bruge almindelig
  SMTP på det lokale net, låst med `tilladte_ip`.
- Exchange viser ofte postkassens eget visningsnavn som afsender, uanset `afsender_navn`.
- Sendte mails ligger i postkassens Sendt post.
- Koden fra Irvold Mail (`IRVMAIL1:`) virker også, men den app har kun Mail.Send.

## Byg

```powershell
C:\Arbejdsmappen\SMTP-Graph-rs\byg.bat
```
Bygger begge programmer og samler dem med scriptet og denne README i `udgivelse\`. Er Inno Setup 6
installeret (`winget install JRSoftware.InnoSetup`), laves også `udgivelse\SMTP-Graph-Setup-<version>.exe`
ud fra `installer\SMTP-Graph.iss` (versionen tages fra `Cargo.toml`, vejledningens PDF kommer med, hvis den findes).
Installeren er ikke signeret, så SmartScreen advarer første gang (»Flere oplysninger« → »Kør alligevel«).
Test uden administrator: `SMTP-Graph-Setup-x.exe /CURRENTUSER /VERYSILENT /DIR=<testmappe>`.
Til udvikling: `smtp-graph-gui.exe --ingen-admin --config <fil> --fane koe --skaermbillede x.png`
gemmer et skærmbillede og lukker (faner: oversigt, ms, modtager, koe, log, om).

## Kundevejledning

`udgivelse\Vejledning-SMTP-Graph.pdf` bygges med `python vejledning\lav_vejledning.py`. Skærmbillederne i
`vejledning\billeder\` tages med `smtp-graph-gui.exe --ingen-admin --demo --config C:\SMTP-Graph\smtp-graph.toml --fane <fane> --skaermbillede <fil>`
(`--demo` viser GUI'en som administrator med eksempeltekster og IP 192.168.1.20; sæt `COMPUTERNAME=SCAN-PC`).

## Licens

MIT – se [LICENSE](LICENSE). Udviklet af Irvold IT & Kommunikation, 4750 Lundby – [www.irvold.dk](https://www.irvold.dk).
