; Installer til SMTP-Graph (Inno Setup 6). Bygges af byg.bat:
;   ISCC.exe /DVersion=0.1.0 installer\SMTP-Graph.iss
; Filerne tages fra udgivelse\, og installeren lægges samme sted.
;
; - Foreslår den mappe, tjenesten SMTPGraph allerede kører fra (ellers C:\SMTP-Graph),
;   så en ny version lægges oven i den gamle.
; - Stopper tjenesten under opdateringen og starter den igen bagefter.
; - Afinstallation fjerner tjenesten og firewall-reglen og spørger, om indstillinger,
;   kø og log også skal slettes (standard: nej).

#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId={{AC68490E-FE95-452C-B1F2-8CA875481ED6}
AppName=SMTP-Graph
AppVersion={#Version}
AppVerName=SMTP-Graph {#Version}
AppPublisher=Irvold
AppPublisherURL=https://irvold.dk
AppSupportURL=https://irvold.dk
AppContact=support@irvold.dk
DefaultDirName={code:StandardMappe}
UsePreviousAppDir=no
DisableDirPage=no
DirExistsWarning=no
DisableProgramGroupPage=yes
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\udgivelse
OutputBaseFilename=SMTP-Graph-Setup-{#Version}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UninstallDisplayName=SMTP-Graph
UninstallDisplayIcon={app}\smtp-graph-gui.exe
VersionInfoVersion={#Version}
VersionInfoCompany=Irvold
VersionInfoDescription=SMTP-Graph installation
VersionInfoProductName=SMTP-Graph
SetupLogging=yes

[Languages]
Name: "dansk"; MessagesFile: "compiler:Languages\Danish.isl"

[Messages]
WelcomeLabel2=Dette installerer [name/ver] på computeren.%n%nSMTP-Graph tager imod mail fra scannere og printere og sender dem videre gennem Microsoft 365.%n%nEr SMTP-Graph allerede installeret, bliver indstillinger, kø og log liggende.

[Tasks]
Name: "skrivebord"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "..\udgivelse\smtp-graph.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\udgivelse\smtp-graph-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\udgivelse\Opsaet-SmtpGraph.ps1"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\udgivelse\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\udgivelse\Vejledning-SMTP-Graph.pdf"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{autoprograms}\SMTP-Graph"; Filename: "{app}\smtp-graph-gui.exe"; Comment: "Opsætning og overvågning af SMTP-Graph"
Name: "{autoprograms}\SMTP-Graph vejledning"; Filename: "{app}\Vejledning-SMTP-Graph.pdf"; Check: HarVejledning
Name: "{autodesktop}\SMTP-Graph"; Filename: "{app}\smtp-graph-gui.exe"; Tasks: skrivebord

[Run]
Filename: "{app}\smtp-graph-gui.exe"; Description: "Start SMTP-Graph nu"; Flags: postinstall nowait skipifsilent runascurrentuser
Filename: "{app}\Vejledning-SMTP-Graph.pdf"; Description: "Åbn vejledningen"; Flags: postinstall shellexec nowait skipifsilent unchecked; Check: HarVejledning

[Code]
const
  Tjeneste = 'SMTPGraph';

var
  StoppedeTjeneste: Boolean;

{ Programmet, tjenesten kører (tom tekst, hvis tjenesten ikke findes). }
function TjenesteExe(): String;
var
  s: String;
  p: Integer;
begin
  Result := '';
  if not RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Services\' + Tjeneste, 'ImagePath', s) then
    Exit;
  s := Trim(s);
  if (Length(s) > 0) and (s[1] = '"') then
  begin
    Delete(s, 1, 1);
    p := Pos('"', s);
    if p > 0 then
      s := Copy(s, 1, p - 1);
  end
  else
  begin
    p := Pos(' --config', s);
    if p > 0 then
      s := Copy(s, 1, p - 1);
  end;
  Result := Trim(s);
end;

{ Kører tjenesten fra mappen? }
function TjenesteIMappe(Mappe: String): Boolean;
var
  e: String;
begin
  e := TjenesteExe();
  Result := (e <> '') and (CompareText(ExtractFileDir(e), RemoveBackslashUnlessRoot(Mappe)) = 0);
end;

function StandardMappe(Param: String): String;
var
  e: String;
begin
  e := TjenesteExe();
  if e <> '' then
    Result := ExtractFileDir(e)
  else
    Result := 'C:\SMTP-Graph';
end;

function HarVejledning(): Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\Vejledning-SMTP-Graph.pdf'));
end;

function NetKommando(Arg: String): Integer;
begin
  if not Exec(ExpandConstant('{sys}\net.exe'), Arg, '', SW_HIDE, ewWaitUntilTerminated, Result) then
    Result := -1;
end;

{ Stop tjenesten, hvis den kører fra den mappe, der installeres i. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  StoppedeTjeneste := False;
  if TjenesteIMappe(WizardDirValue) then
  begin
    Log('Stopper tjenesten ' + Tjeneste);
    StoppedeTjeneste := NetKommando('stop ' + Tjeneste) = 0;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and StoppedeTjeneste then
  begin
    Log('Starter tjenesten ' + Tjeneste);
    if NetKommando('start ' + Tjeneste) <> 0 then
      SuppressibleMsgBox('Tjenesten SMTPGraph kunne ikke startes igen. Start den fra SMTP-Graph-programmet, og se loggen.',
        mbError, MB_OK, IDOK);
  end;
end;

procedure SletFil(Fil: String);
begin
  if FileExists(Fil) then
    DeleteFile(Fil);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  app: String;
  rc: Integer;
begin
  app := ExpandConstant('{app}');
  if (CurUninstallStep = usUninstall) and TjenesteIMappe(app) and FileExists(app + '\smtp-graph.exe') then
  begin
    { Stopper og fjerner tjenesten og firewall-reglen. }
    Exec(app + '\smtp-graph.exe', 'afinstaller', app, SW_HIDE, ewWaitUntilTerminated, rc);
  end;

  if CurUninstallStep = usPostUninstall then
  begin
    if FileExists(app + '\smtp-graph.toml') or DirExists(app + '\koe') or DirExists(app + '\log') then
    begin
      if SuppressibleMsgBox('Skal indstillinger, kø og log i ' + app + ' også slettes?' + #13#10#13#10 +
        'Vælg Nej, hvis SMTP-Graph skal installeres igen med de samme indstillinger og den samme forbindelse til Microsoft 365.',
        mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
      begin
        SletFil(app + '\smtp-graph.toml');
        SletFil(app + '\graph-login.json');
        SletFil(app + '\cert-rsa.pem');
        SletFil(app + '\noegle-rsa.pem');
        SletFil(app + '\cert.pem');
        SletFil(app + '\noegle.pem');
        SletFil(app + '\smtp-graph-fejl.txt');
        DelTree(app + '\koe', True, True, True);
        DelTree(app + '\log', True, True, True);
      end;
    end;
    RemoveDir(app);
  end;
end;
