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
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    { Launch at login and the network check at login are Kairo's, not the installer's. }
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Kairo');
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'KairoNetworkCheck');
  end;
end;
