; The installer: for the current user only (no administrator), into
; %LOCALAPPDATA%\Programs\Kairo, so Kairo can update itself in place.
; Built by scripts\package-windows.ps1 with /DAppVersion /DSourceDir /DOutputName.

[Setup]
AppId={{6C1B6E5D-4B1C-4F0E-9C32-5A0A6F3D1B10}
AppName=Kairo
AppVersion={#AppVersion}
AppPublisher=Kairo
AppPublisherURL=https://github.com/daoleno/kairo
DefaultDirName={autopf}\Kairo
DefaultGroupName=Kairo
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputBaseFilename={#OutputName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\assets\kairo.ico
UninstallDisplayIcon={app}\kairo.exe
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no
MinVersion=10.0.17763

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Kairo"; Filename: "{app}\kairo.exe"

[Run]
Filename: "{app}\kairo.exe"; Description: "Launch Kairo"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; What an update leaves aside.
Type: files; Name: "{app}\*.old*"

[Code]
{ The user name as Kairo's task and folder names spell it: letters, digits, - and _ only. }
function KairoUserName(): String;
var
  s: String;
  i: Integer;
  c: Char;
begin
  s := GetUserNameString();
  Result := '';
  for i := 1 to Length(s) do
  begin
    c := s[i];
    if ((c >= 'a') and (c <= 'z')) or ((c >= 'A') and (c <= 'Z')) or ((c >= '0') and (c <= '9')) or (c = '-') or (c = '_') then
      Result := Result + c
    else
      Result := Result + '_';
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  rc: Integer;
begin
  { Windows TUN: if this user gave Kairo the permission (a SYSTEM task and
    administrator-owned folders under ProgramData), take it away while kairo.exe
    is still here. It asks for administrator approval (UAC); if that is declined
    the uninstall goes on and the leftovers do nothing without kairo.exe. }
  if CurUninstallStep = usUninstall then
  begin
    if FileExists(ExpandConstant('{commonappdata}\Kairo\core\grant-KairoCore-') + KairoUserName() + '.json') then
      ShellExec('runas', ExpandConstant('{app}\kairo.exe'), '--revoke-tun --user ' + KairoUserName(), '', SW_HIDE, ewWaitUntilTerminated, rc);
  end;
  if CurUninstallStep = usPostUninstall then
  begin
    { Launch at login and the network check at login are Kairo's, not the installer's. }
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Kairo');
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'KairoNetworkCheck');
  end;
end;
