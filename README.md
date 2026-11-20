# SMTP-Graph

*English · [Dansk](README.da.md)*

SMTP-Graph accepts mail over SMTP, for example from a scanner or multifunction printer, and
sends it on through **Microsoft Graph**. Use it when Microsoft 365 no longer accepts SMTP with a
username and password (basic auth) and the device cannot do OAuth itself.

```
Scanner ──SMTP──> smtp-graph ──HTTPS (Graph)──> Microsoft 365 ──> recipient
                      │
                      └─ queue  (mail waits here if Microsoft does not answer)
```

It runs as a **Windows service** with a graphical setup tool and installer, or as a **Linux
systemd service** (command line only). It is written in Rust and ships as single binaries with no
runtime dependencies.

> **Language note:** the user interface, the customer guide, the command names and the
> configuration keys are in Danish (for example `tilladte_ip` = allowed IPs, `koe` = queue,
> `opsaet` = set up). This README explains them in English.

## Download

Get the latest build from [Releases](https://github.com/irvold/smtp-graph/releases):

| File | |
|---|---|
| `SMTP-Graph-Setup-<version>.exe` | Windows installer (service + GUI). Not code-signed, so SmartScreen warns the first time: *More info → Run anyway* |
| `smtp-graph-<version>-linux-x86_64.tar.gz` | Linux, ordinary PCs and servers (static musl binary) |
| `smtp-graph-<version>-linux-aarch64.tar.gz` | Linux on ARM64, e.g. Raspberry Pi 4/5 with a 64-bit OS |
| `Vejledning-SMTP-Graph.pdf` | Step-by-step first-time setup guide for customers (Danish) |

## Features

- **SMTP** on port 25/587 with STARTTLS, and implicit TLS (typically port 465). A self-signed RSA
  certificate is generated automatically.
- **Older scanners on Windows:** if the modern TLS stack (rustls) cannot negotiate with the device,
  the same connection is handed over to Windows' own TLS (Schannel). This makes devices that only
  offer RSA key exchange work, e.g. the HP PageWide 477.
- **Login is optional** (AUTH PLAIN/LOGIN). Without login, access is controlled by an IP allow list (`tilladte_ip`).
- **Large mail:** messages up to about 3 MB are sent as raw MIME. Larger messages are sent as a
  draft plus a chunked upload, with up to 150 MB per attachment.
- **Fixed sender:** the sender is always the one locked mailbox. The device's own address can be
  put in Reply-To.
- **Envelope recipients only:** only the envelope recipients (RCPT TO) receive the message.
  Recipients that are not in the headers are sent as Bcc.
- **Recipient lock:** `tilladte_modtagere` can restrict delivery, e.g. to the company's own domain only.
- **Disk queue with retries** after 1, 2, 5, 10 and 30 minutes, then hourly for 48 hours. Mail that
  is given up on goes to `koe\fejlet`, and `fejl_til` is notified.
- **Fallback relay** (`[reserve]`): if Microsoft cannot send, mail goes through another SMTP server
  instead, e.g. Gmail with an app password. See [Fallback relay](#fallback-relay-gmail-or-any-smtp-server).
- **Dry-run mode** (`toerlob = true`): mail is stored in `koe\toerlob` instead of being sent.
- **Daily log files** in `log\`, kept for 60 days. On Linux the log also goes to the journal.

## Microsoft 365: how access works

SMTP-Graph never stores a user's password. There are two ways to connect it to a tenant. Both
start with a browser login through a **shared multi-tenant app** (a public client with PKCE and no
secret). That app identifies the program to Microsoft, much like "Sign in with Google" does for a
website.

| | Administrator creates an app (recommended) | Sign in as the mailbox |
|---|---|---|
| Who signs in | An administrator in the customer's tenant | The account that sends (needs a licence) |
| What happens | The program creates the customer's **own** app registration and a client secret (24 months), and locks the app to the one mailbox with Exchange *RBAC for Applications* | A refresh token is stored in `graph-login.json` and renewed by the service every day |
| Sends through | The customer's own app (client credentials) | The shared app, on the user's behalf |
| Shared mailbox | Yes (can be created as part of the setup) | Only through a user with Send As and Full Access |
| Must be repeated | When the secret expires (the log warns 30 days ahead) | After a password change, a block, or 90 days without renewal |

The administrator path calls Exchange Online's REST endpoint `adminapi/beta/{tenant}/InvokeCommand`,
which Microsoft's own PowerShell module uses. It is not officially documented. If it stops
working, the PowerShell route (`Opsaet-SmtpGraph.ps1`, PowerShell 7) does the same job.

**Using your own shared app:** the app id built into `oauth::FAELLES_APP` belongs to Irvold. To use
your own, run `Opret-FaellesApp.ps1` once in your own tenant, then set `graph.faelles_app` in the
configuration or change the constant. Customers see the app as an *unverified publisher* until the
publisher is verified. Administrators can always consent to it; ordinary users can only if the
tenant allows user consent.

## Fallback relay (Gmail or any SMTP server)

If Microsoft 365 cannot send (an outage, an expired secret, a revoked app), SMTP-Graph can hand the
mail to another SMTP server instead.

```toml
[reserve]
server = "smtp.gmail.com"
port = 587
tls = "starttls"            # "starttls" (587), "tls" (465) or "ingen" (none, local relays only)
bruger = "company@gmail.com"
adgangskode = "abcd efgh ijkl mnop"
fra = "company@gmail.com"   # sender; empty = bruger
efter_minutter = 15         # use the fallback once a mail has waited this long (0 = at once)
```

- **When it is used:** a temporary failure (network, throttling, a Microsoft outage) is retried as
  usual until the mail has waited `efter_minutter`. If Microsoft rejects the mail or the
  credentials outright, the fallback is used immediately. If the fallback also fails, the normal
  retry schedule continues.
- **Sender:** the From header is rewritten to the fallback account, since Gmail only allows its own
  address. The Bcc header is removed, and every envelope recipient still gets the mail.
- **Notification:** `fejl_til` gets at most one notice every 6 hours saying that the fallback is in use.
- **Gmail:** turn on 2-step verification for the account and create an app password at
  <https://myaccount.google.com/apppasswords>. The normal account password does not work. The
  spaces in the app password are optional. Gmail allows about 25 MB per message.
- **Testing:** in the GUI, use *Microsoft 365 → Reserve*. From the command line, run
  `smtp-graph tjek`, which also tests the fallback login, and
  `smtp-graph test --til you@example.com --reserve`.

## Windows

Run `SMTP-Graph-Setup-<version>.exe`.

- It installs to `C:\SMTP-Graph`, or to the folder the `SMTPGraph` service already runs from.
  Configuration, queue and log live next to the program.
- It adds Start menu and desktop shortcuts and starts `smtp-graph-gui.exe`, which asks for
  administrator rights (UAC).
- **Upgrades:** installing a new version on top of the old one stops the service, replaces the
  files and starts it again. Settings, queue and log are kept.
- **Uninstalling** (Settings → Apps) removes the service and the firewall rule, and asks whether
  settings, queue and log should be deleted as well.

The GUI tabs:

| Tab | |
|---|---|
| **Oversigt** (Overview) | How to configure the scanner: the PC's IP addresses, ports and encryption, login, who may connect. Also the Microsoft 365 status, when the secret expires, the queue and the latest log lines |
| **Microsoft 365** | Choose one of the two access methods, test the login and send a test mail. *Advanced* has the shared app id, the PowerShell route and manual entry |
| **Modtager (SMTP)** (Receiver) | Ports, TLS engine, allowed IPs, users, allowed recipients, sender name, failure notifications, dry run. Saving keeps the comments in the file |
| **Kø** (Queue) | Waiting, failed and dry-run mail: send now, open the .eml, delete |
| **Log** | Today's log with search and a warnings-only filter |
| **Om** (About) | Version and developer |

Along the top are the service state and the buttons Start, Stop, Restart, Install and Uninstall.

### Windows from the command line

```powershell
C:\SMTP-Graph\smtp-graph.exe ny-config
```
This writes `smtp-graph.toml`, a configuration file with comments. Set `tilladte_ip` to the scanner's address.

```powershell
C:\SMTP-Graph\smtp-graph.exe opsaet --postkasse scanner@example.com --navn "Example office" --opret-delt
```
This signs in as an administrator in the browser, creates the locked app and writes it into the configuration.

```powershell
C:\SMTP-Graph\smtp-graph.exe test --til you@example.com --stor
```
This sends a test mail; `--stor` adds a 5 MB attachment. New Exchange permissions can take up to 2 hours to apply.

```powershell
C:\SMTP-Graph\smtp-graph.exe installer
```
This creates the `SMTPGraph` service (LocalSystem, automatic start, restart on failure). It opens
the firewall for the listening ports, from the allowed IPs only, and restricts the configuration
file to SYSTEM and Administrators.

## Linux

The Linux build is the same engine without the GUI, running as a systemd service. The full
step-by-step guide is in [`linux/LAES-MIG.txt`](linux/LAES-MIG.txt) (Danish), which is included in the package.

```sh
tar xzf smtp-graph-*-linux-x86_64.tar.gz && cd smtp-graph-*-linux-x86_64
sudo ./smtp-graph installer
```
This copies the program to `/usr/local/bin`, puts the data in `/var/lib/smtp-graph` and creates the
system user `smtp-graph`.

```sh
sudo nano /var/lib/smtp-graph/smtp-graph.toml
```
Set at least `tilladte_ip`.

```sh
sudo smtp-graph opsaet --postkasse scanner@example.com --navn "Example office" --opret-delt
```
This connects the mailbox in Microsoft 365.

```sh
sudo smtp-graph installer
```
Run the installer again to start the service once Microsoft 365 is set up.

- **Hardened service:** it runs as the `smtp-graph` user with only `CAP_NET_BIND_SERVICE`, under
  `ProtectSystem=strict`. The binary is owned by root, so the service cannot modify it.
- **Started only when ready:** the service is enabled at boot only once it can start, so an
  unconfigured install does not restart in a loop.
- **sudo runs as the service user:** commands such as `tjek`, `test`, `opsaet`, `login` and `koe`
  switch to `smtp-graph` by themselves when run with sudo, so new files get the right owner.
- **Headless login:** the program prints the login URL. Sign in in a browser on any computer, then
  paste the address the browser ends up on (`http://127.0.0.1:…/?code=…`) back into the terminal.
- **Firewall:** if ufw is enabled, it is opened for the listening ports from the allowed IPs. Other
  firewalls must be opened by hand.
- **No Windows TLS fallback on Linux.** Devices that rustls cannot talk to must send unencrypted on
  the local network, locked down with `tilladte_ip`. Some devices, such as the HP PageWide 477, try
  STARTTLS whenever it is offered, even with "secure connection" turned off; set
  `starttls = false` for those.
- **Uninstalling:** `sudo smtp-graph afinstaller` removes the service and the firewall rules. Data
  and binary are left in place, and the command prints how to remove them.

## Commands

| Command | |
|---|---|
| *(none)* | Run in the foreground (Ctrl+C stops) |
| `ny-config` | Write a configuration file with explanations |
| `tjek` | Show the configuration and test the login to Microsoft and the fallback relay |
| `test --til <addr> [--stor] [--reserve]` | Send a test mail directly (`--stor`: 5 MB attachment, `--reserve`: through the fallback relay) |
| `opsaet --postkasse <addr> --navn <name> [--opret-delt] [--kun-send]` | Administrator setup in the browser |
| `login [--send-fra <addr>]` | Sign in as the mailbox (the "user" method) |
| `koe` | List waiting and failed mail |
| `installer` / `afinstaller` | Install or remove the service (Windows service or systemd) |

`--config <file>` selects another configuration file. The default is `smtp-graph.toml` next to the
program, or `/var/lib/smtp-graph/smtp-graph.toml` on Linux.

## Configuring the scanner

| Setting | Value |
|---|---|
| SMTP server | The IP address of the machine running SMTP-Graph |
| Port | 25 |
| Encryption | None or STARTTLS. Turn off "validate server certificate", because the certificate is self-signed |
| Login | None (access is controlled by `tilladte_ip`), unless `[[smtp.bruger]]` users are configured |
| From address | Anything; mail is always sent from the locked mailbox |

## Good to know

- **TLS 1.0/1.1** are disabled in recent Windows 11 builds. A device that can only do those must
  use plain SMTP on the local network.
- **Sender name:** Exchange often shows the mailbox's own display name as the sender, whatever
  `afsender_navn` says.
- **Sent Items:** sent mail appears in the mailbox's Sent Items folder.
- **Logs:** they never contain passwords. A connection that ends without mail
  logs its command sequence (for example `EHLO 250, STARTTLS 220, …`) to help with troubleshooting.

## Building

**Windows** (Rust stable, MSVC):
```powershell
.\byg.bat
```
This builds both programs and collects them in `udgivelse\`. If
[Inno Setup 6](https://jrsoftware.org/isinfo.php) is installed (`winget install JRSoftware.InnoSetup`),
it also builds `udgivelse\SMTP-Graph-Setup-<version>.exe` from `installer\SMTP-Graph.iss`.

**Linux** (cross-compiled from WSL with [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) and zig):
```sh
bash linux/byg-linux.sh
```
This builds static x86_64 and aarch64 binaries (`--no-default-features`, i.e. without the GUI) and
packs them into `udgivelse/linux/`.

**Tests:**
```powershell
cargo test
```
The GUI tests use `egui_kittest`.

**Customer guide:** the guide is built by `python vejledning/lav_vejledning.py` (reportlab). Its
screenshots come from `smtp-graph-gui.exe --ingen-admin --demo --fane <tab> --skaermbillede <file>`.

## Status

SMTP-Graph is in use with an HP PageWide 477, sending through a real Microsoft 365 tenant, on both
Windows and Linux. Some things have not been tested yet:

- the *sign in as the mailbox* method, against a real tenant
- the aarch64 build, on real hardware
- headless login by pasting the address, against a real tenant

## License

MIT, see [LICENSE](LICENSE). Developed by Irvold IT & Kommunikation, 4750 Lundby,
[www.irvold.dk](https://www.irvold.dk).
