<#
.SYNOPSIS
    Sætter Microsoft 365 op, så SMTP-Graph kan sende scannerens mails gennem Microsoft Graph.

.DESCRIPTION
    Scriptet opretter en app-registrering i kundens egen Microsoft 365, giver den lov til at sende
    fra ÉN postkasse (og kun den) og laver en opsætningskode til smtp-graph.toml.

    Appen får IKKE brede Graph-tilladelser i Entra ID. Adgangen gives i Exchange Online med
    "RBAC for Applications", afgrænset til postkassen:
      - Application Mail.Send       – mails op til ca. 3 MB
      - Application Mail.ReadWrite  – større mails (kladde + upload af vedhæftninger, op til 150 MB)
    Brug en delt postkasse kun til scanneren (fx scanner@kunde.dk), så ReadWrite ikke giver adgang til andet.

    Kræver PowerShell 7 og login som administrator i kundens Microsoft 365
    (Global administrator, eller Application administrator + Exchange administrator).

.PARAMETER Postkasse
    Postkassen, scanneren sender fra, fx scanner@kunde.dk. En delt postkasse koster ingen licens.

.PARAMETER Navn
    Stedets navn, fx »kunde.dk kontor«. Bruges i appens navn, så den er let at finde igen.

.PARAMETER SendSom
    Valgfri. En anden adresse, mailene skal stå som sendt fra. Postkassen får »Send som« på den.

.PARAMETER KunSend
    Giv kun Mail.Send (ingen mails over ca. 3 MB).

.PARAMETER Config
    Skriv koden direkte ind i denne smtp-graph.toml (linjen kode = "…") i stedet for at vise den.

.PARAMETER KodeFil
    Gem koden i en fil i stedet for at vise den. Filen indeholder hemmeligheden; slet den bagefter.

.PARAMETER Maaneder
    Hvor længe klienthemmeligheden gælder. Standard 24. SMTP-Graph advarer i loggen 30 dage før.

.PARAMETER Forny
    Lav kun en ny klienthemmelighed til den eksisterende app.

.PARAMETER OpretDelt
    Opret postkassen som delt postkasse, hvis den ikke findes.

.EXAMPLE
    ./Opsaet-SmtpGraph.ps1 -Postkasse scanner@kunde.dk -Navn "kunde.dk kontor" -OpretDelt -Config C:\SMTP-Graph\smtp-graph.toml

.EXAMPLE
    ./Opsaet-SmtpGraph.ps1 -Postkasse scanner@kunde.dk -Navn "kunde.dk kontor" -Forny -Config C:\SMTP-Graph\smtp-graph.toml
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Postkasse,
    [Parameter(Mandatory)] [string] $Navn,
    [string] $SendSom = '',
    [switch] $KunSend,
    [string] $Config = '',
    [string] $KodeFil = '',
    [ValidateRange(1, 24)] [int] $Maaneder = 24,
    [switch] $Forny,
    [switch] $OpretDelt
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Skriv([string] $t, [string] $farve = 'Gray') { Write-Host $t -ForegroundColor $farve }
function Trin([string] $t) { Write-Host ''; Write-Host "» $t" -ForegroundColor Cyan }

if ($PSVersionTable.PSVersion.Major -lt 7) {
    throw 'Scriptet kræver PowerShell 7. Hent det på https://aka.ms/powershell og kør scriptet med pwsh.'
}
if ($Postkasse -notmatch '^[^@\s]+@[^@\s]+\.[^@\s]+$') { throw "»$Postkasse« er ikke en gyldig mailadresse." }
$Postkasse = $Postkasse.ToLowerInvariant()
if ($SendSom) {
    if ($SendSom -notmatch '^[^@\s]+@[^@\s]+\.[^@\s]+$') { throw "»$SendSom« er ikke en gyldig mailadresse." }
    $SendSom = $SendSom.ToLowerInvariant()
}
if ($Config -and -not (Test-Path $Config)) { throw "Filen $Config findes ikke. Lav den med: smtp-graph.exe ny-config" }
$appNavn = "SMTP-Graph – $Navn"
$scopeNavn = "SMTP-Graph – $Postkasse"
$roller = @('Application Mail.Send')
if (-not $KunSend) { $roller += 'Application Mail.ReadWrite' }

# ---------------------------------------------------------------- moduler
Trin 'Tjekker PowerShell-moduler'
foreach ($m in 'Microsoft.Graph.Authentication', 'Microsoft.Graph.Applications', 'ExchangeOnlineManagement') {
    if (-not (Get-Module -ListAvailable -Name $m)) {
        Skriv "  Installerer $m (kun for din bruger) …"
        Install-Module $m -Scope CurrentUser -Force -AllowClobber
    }
}
Import-Module Microsoft.Graph.Authentication, Microsoft.Graph.Applications, ExchangeOnlineManagement

# ---------------------------------------------------------------- Entra ID
Trin 'Log ind i Microsoft 365 (Entra ID)'
# Login i den almindelige browser. Windows' eget login-vindue (WAM) gemmer sig let bag andre vinduer.
try { Set-MgGraphOption -DisableLoginByWAM $true } catch { }
Skriv '  Din browser åbner nu. Log ind som administrator.'
Connect-MgGraph -Scopes 'Application.ReadWrite.All' -NoWelcome
$ctx = Get-MgContext
$tenant = $ctx.TenantId
Skriv "  Logget ind som $($ctx.Account) i tenant $tenant"

$app = Get-MgApplication -Filter "displayName eq '$($appNavn.Replace("'", "''"))'" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($Forny) {
    if (-not $app) { throw "Der findes ingen app med navnet »$appNavn«. Kør scriptet uden -Forny." }
    Skriv "  Fornyer hemmeligheden til den eksisterende app $($app.AppId)"
} elseif ($app) {
    Skriv "  Appen »$appNavn« findes allerede ($($app.AppId)). Den genbruges." 'Yellow'
} else {
    Trin "Opretter appen »$appNavn«"
    $app = New-MgApplication -DisplayName $appNavn -SignInAudience 'AzureADMyOrg' `
        -Notes "Bruges af SMTP-Graph ($Navn) til at sende scannermail fra $Postkasse. Adgangen er afgrænset i Exchange Online (RBAC for Applications)."
    Skriv "  App-id: $($app.AppId)"
}

$sp = Get-MgServicePrincipal -Filter "appId eq '$($app.AppId)'" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $sp) {
    $sp = New-MgServicePrincipal -AppId $app.AppId
    Skriv '  Tjenesteprincipal oprettet'
}

Trin 'Laver en klienthemmelighed'
$udloeber = (Get-Date).AddMonths($Maaneder)
$secret = Add-MgApplicationPassword -ApplicationId $app.Id -PasswordCredential @{
    DisplayName = "SMTP-Graph $Navn $(Get-Date -Format 'yyyy-MM-dd')"
    EndDateTime = $udloeber
}
Skriv "  Gælder til $($udloeber.ToString('d. MMMM yyyy', [cultureinfo]'da-DK'))"
$gamle = @((Get-MgApplication -ApplicationId $app.Id).PasswordCredentials | Where-Object { $_.KeyId -ne $secret.KeyId })
if ($gamle.Count -gt 0) {
    Skriv "  Der er $($gamle.Count) ældre hemmelighed(er) på appen. Slet dem i Entra, når den nye virker (smtp-graph.exe tjek)." 'Yellow'
}

# ---------------------------------------------------------------- Exchange Online
if (-not $Forny) {
    Trin 'Log ind i Exchange Online'
    Skriv '  Din browser åbner igen. Log ind med samme konto.'
    $exoArgs = @{ ShowBanner = $false; UserPrincipalName = $ctx.Account }
    if ((Get-Command Connect-ExchangeOnline).Parameters.ContainsKey('DisableWAM')) { $exoArgs.DisableWAM = $true }
    Connect-ExchangeOnline @exoArgs

    $mbx = Get-EXOMailbox -Identity $Postkasse -ErrorAction SilentlyContinue
    if (-not $mbx -and $OpretDelt) {
        Trin "Opretter den delte postkasse $Postkasse"
        New-Mailbox -Shared -Name "Scanner $Navn" -DisplayName "Scanner $Navn" -PrimarySmtpAddress $Postkasse | Out-Null
        Start-Sleep -Seconds 20
        $mbx = Get-EXOMailbox -Identity $Postkasse -ErrorAction SilentlyContinue
    }
    if (-not $mbx) { throw "Postkassen $Postkasse findes ikke i denne Microsoft 365. Opret den (gerne som delt postkasse), eller kør scriptet med -OpretDelt." }
    Skriv "  Postkasse: $($mbx.DisplayName) ($($mbx.RecipientTypeDetails))"
    if ($mbx.RecipientTypeDetails -eq 'UserMailbox') {
        Skriv '  Bemærk: det er en personlig postkasse. Med Mail.ReadWrite kan appen læse den. Brug hellere en delt postkasse kun til scanneren.' 'Yellow'
    }
    $primaer = $mbx.PrimarySmtpAddress.ToString().ToLowerInvariant()
    if ($primaer -ne $Postkasse) {
        Skriv "  $Postkasse er et alias. Der sendes fra den primære adresse $primaer." 'Yellow'
        $Postkasse = $primaer
        $scopeNavn = "SMTP-Graph – $Postkasse"
    }

    Trin 'Giver appen adgang til postkassen (og kun den)'
    if (-not (Get-ServicePrincipal -Identity $sp.Id -ErrorAction SilentlyContinue)) {
        New-ServicePrincipal -AppId $app.AppId -ObjectId $sp.Id -DisplayName $appNavn | Out-Null
        Skriv '  Appen er registreret i Exchange'
    }
    if (-not (Get-ManagementScope -Identity $scopeNavn -ErrorAction SilentlyContinue)) {
        New-ManagementScope -Name $scopeNavn -RecipientRestrictionFilter "PrimarySmtpAddress -eq '$Postkasse'" | Out-Null
        Skriv "  Afgrænsning oprettet: kun $Postkasse"
    }
    foreach ($rolle in $roller) {
        $findes = Get-ManagementRoleAssignment -RoleAssignee $sp.Id -Role $rolle -ErrorAction SilentlyContinue |
            Where-Object { $_.CustomResourceScope -eq $scopeNavn }
        if (-not $findes) {
            New-ManagementRoleAssignment -App $sp.Id -Role $rolle -CustomResourceScope $scopeNavn | Out-Null
            Skriv "  Rollen »$rolle« er tildelt"
        } else { Skriv "  Rollen »$rolle« fandtes allerede" }
    }

    if ($SendSom -and $SendSom -ne $Postkasse) {
        Trin "Giver $Postkasse lov til at sende som $SendSom"
        $modt = Get-Recipient -Identity $SendSom -ErrorAction SilentlyContinue
        if (-not $modt) { throw "$SendSom findes ikke i denne Microsoft 365." }
        $har = Get-RecipientPermission -Identity $SendSom -Trustee $Postkasse -ErrorAction SilentlyContinue | Where-Object { $_.AccessRights -contains 'SendAs' }
        if (-not $har) {
            Add-RecipientPermission -Identity $SendSom -Trustee $Postkasse -AccessRights SendAs -Confirm:$false | Out-Null
            Skriv '  »Send som« er givet'
        } else { Skriv '  »Send som« fandtes allerede' }
    }

    Trin 'Tjekker adgangen'
    $test = @(Test-ServicePrincipalAuthorization -Identity $sp.Id -Resource $Postkasse | Where-Object { $roller -contains $_.RoleName })
    foreach ($rolle in $roller) {
        $r = $test | Where-Object { $_.RoleName -eq $rolle } | Select-Object -First 1
        if ($r -and $r.InScope) { Skriv "  OK: $rolle på $Postkasse" 'Green' }
        else { Skriv "  Advarsel: $rolle vises ikke endnu. Det kan tage op til 2 timer." 'Yellow' }
    }

    # Den brede Entra-tilladelse må ikke være der, for så gælder afgrænsningen ikke.
    $graph = Get-MgServicePrincipal -Filter "appId eq '00000003-0000-0000-c000-000000000000'"
    $bred = Get-MgServicePrincipalAppRoleAssignment -ServicePrincipalId $sp.Id -ErrorAction SilentlyContinue | Where-Object { $_.ResourceId -eq $graph.Id }
    if ($bred) { Skriv '  ADVARSEL: appen har også Graph-tilladelser i Entra ID. Fjern dem under Virksomhedsprogrammer → Tilladelser, ellers kan den bruge alle postkasser.' 'Red' }

    Disconnect-ExchangeOnline -Confirm:$false | Out-Null
}
Disconnect-MgGraph | Out-Null

# ---------------------------------------------------------------- opsætningskode
$kode = 'SMTPGRAPH1:' + [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes((@{
    tenant    = $tenant
    client_id = $app.AppId
    secret    = $secret.SecretText
    postkasse = $Postkasse
    from      = $(if ($SendSom) { $SendSom } else { $Postkasse })
    udloeber  = $udloeber.ToString('yyyy-MM-dd')
} | ConvertTo-Json -Compress)))

Trin 'Færdig'
if ($Config) {
    $tekst = Get-Content -Raw -Encoding utf8 $Config
    if ($tekst -match '(?m)^\s*kode\s*=') {
        $tekst = [regex]::Replace($tekst, '(?m)^\s*kode\s*=.*$', "kode = `"$kode`"", 1)
    } else {
        $tekst = [regex]::Replace($tekst, '(?m)^\[graph\]\s*$', "[graph]`nkode = `"$kode`"", 1)
    }
    [IO.File]::WriteAllText((Resolve-Path $Config), $tekst, [Text.UTF8Encoding]::new($false))
    Skriv "  Koden er skrevet ind i $Config" 'Green'
    Skriv '  Genstart tjenesten (Restart-Service SMTPGraph) og kør: smtp-graph.exe tjek'
} elseif ($KodeFil) {
    [IO.File]::WriteAllText($KodeFil, $kode)
    Skriv "  Koden er gemt i $KodeFil. Den indeholder hemmeligheden – slet filen, når koden er sat ind." 'Yellow'
} else {
    Skriv '  Sæt denne linje ind under [graph] i smtp-graph.toml:'
    Write-Host ''
    Write-Host "kode = `"$kode`"" -ForegroundColor White
    Write-Host ''
    try { Set-Clipboard -Value "kode = `"$kode`""; Skriv '  (Linjen er også kopieret til udklipsholderen.)' } catch { }
}
Skriv '  Nye rettigheder i Exchange kan tage op til 2 timer. Indtil da venter mails i køen og prøves igen.' 'Yellow'
