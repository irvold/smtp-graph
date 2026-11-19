<#
.SYNOPSIS
    Opretter Irvolds fælles login-app til SMTP-Graph (køres ÉN gang i Irvolds egen Microsoft 365).

.DESCRIPTION
    SMTP-Graph lader en administrator eller en bruger hos kunden logge ind i browseren. Det login
    sker gennem én fælles, multi-tenant app i Irvolds Microsoft 365. Appen har ingen hemmelighed
    (offentlig klient med PKCE) og får kun rettigheder hos en kunde, når kunden selv logger ind og
    godkender dem.

    Rettigheder (delegerede, altså kun på vegne af den, der logger ind):
      - Administrator-opsætning: Application.ReadWrite.All (opretter kundens egen app) og
        Exchange.Manage (låser appen til postkassen i Exchange Online).
      - Login som postkassen: Mail.Send, Mail.Send.Shared, Mail.ReadWrite, Mail.ReadWrite.Shared.
      - Altid: openid, profile, offline_access, User.Read.

    Scriptet viser app-id'et til sidst. Det indsættes i SMTP-Graph under
    Microsoft 365 → Avanceret → Irvolds fælles login-app (eller bygges ind i programmet).

.EXAMPLE
    pwsh ./Opret-FaellesApp.ps1
#>
[CmdletBinding()]
param([string] $Navn = 'SMTP-Graph (Irvold)')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'Kør scriptet med PowerShell 7 (pwsh).' }

foreach ($m in 'Microsoft.Graph.Authentication', 'Microsoft.Graph.Applications') {
    if (-not (Get-Module -ListAvailable -Name $m)) { Install-Module $m -Scope CurrentUser -Force -AllowClobber }
}
Import-Module Microsoft.Graph.Authentication, Microsoft.Graph.Applications
try { Set-MgGraphOption -DisableLoginByWAM $true } catch { }
Write-Host 'Log ind som administrator i IRVOLDS Microsoft 365 (ikke kundens).' -ForegroundColor Cyan
Connect-MgGraph -Scopes 'Application.ReadWrite.All' -NoWelcome
Write-Host "Logget ind som $((Get-MgContext).Account)"

function Hent-Rettigheder([string] $resourceAppId, [string[]] $navne) {
    $sp = Get-MgServicePrincipal -Filter "appId eq '$resourceAppId'"
    if (-not $sp) { throw "Tjenesten $resourceAppId findes ikke i denne tenant." }
    $adgang = foreach ($n in $navne) {
        $s = $sp.Oauth2PermissionScopes | Where-Object { $_.Value -eq $n }
        if (-not $s) { throw "Rettigheden $n findes ikke på $($sp.DisplayName)." }
        @{ Id = $s.Id; Type = 'Scope' }
    }
    @{ ResourceAppId = $resourceAppId; ResourceAccess = @($adgang) }
}

$graph = Hent-Rettigheder '00000003-0000-0000-c000-000000000000' @(
    'openid', 'profile', 'offline_access', 'User.Read',
    'Mail.Send', 'Mail.Send.Shared', 'Mail.ReadWrite', 'Mail.ReadWrite.Shared',
    'Application.ReadWrite.All')
$exo = Hent-Rettigheder '00000002-0000-0ff1-ce00-000000000000' @('Exchange.Manage')

$app = Get-MgApplication -Filter "displayName eq '$($Navn.Replace("'", "''"))'" | Select-Object -First 1
$param = @{
    DisplayName            = $Navn
    SignInAudience         = 'AzureADMultipleOrgs'
    IsFallbackPublicClient = $true
    PublicClient           = @{ RedirectUris = @('http://127.0.0.1', 'http://localhost') }
    RequiredResourceAccess = @($graph, $exo)
    Notes                  = 'Login til SMTP-Graph (scanner-mail via Microsoft Graph) hos Irvolds kunder. Offentlig klient med PKCE, ingen hemmelighed.'
}
if ($app) {
    Update-MgApplication -ApplicationId $app.Id @param
    Write-Host "Appen »$Navn« fandtes og er opdateret." -ForegroundColor Yellow
} else {
    $app = New-MgApplication @param
    Write-Host "Appen »$Navn« er oprettet." -ForegroundColor Green
}
if (-not (Get-MgServicePrincipal -Filter "appId eq '$($app.AppId)'")) {
    New-MgServicePrincipal -AppId $app.AppId | Out-Null
}
Disconnect-MgGraph | Out-Null

Write-Host ''
Write-Host "App-id: $($app.AppId)" -ForegroundColor White
try { Set-Clipboard -Value $app.AppId; Write-Host '(kopieret til udklipsholderen)' } catch { }
Write-Host ''
Write-Host 'Kunder ser appen som »ikke bekræftet udgiver«, indtil Irvold er bekræftet udgiver (Microsoft Partner-id under Branding).' -ForegroundColor Yellow
Write-Host 'Administratorer kan altid godkende den; almindelige brugere kun, hvis kundens indstillinger tillader det.' -ForegroundColor Yellow
