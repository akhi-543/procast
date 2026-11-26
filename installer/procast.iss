; procast installer (Inno Setup 6). Per-user: no admin, installs to %LOCALAPPDATA%\Programs\procast.
; Build: cargo build --release, then (cd exthost && npm ci), then
;   ISCC.exe /DAppVersion=0.2.0 installer\procast.iss
; Output: target\installer\procast-setup-<version>.exe

#ifndef AppVersion
  #define AppVersion "0.2.0"
#endif

[Setup]
AppId={{8E4C2B71-5A0D-4F43-9C59-7D1E3A6B2F10}
AppName=procast
AppVersion={#AppVersion}
AppPublisher=Akhilesh Teja Goli
AppPublisherURL=https://github.com/akhi-543/procast
DefaultDirName={localappdata}\Programs\procast
DefaultGroupName=procast
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
OutputDir=..\target\installer
OutputBaseFilename=procast-setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
LicenseFile=..\LICENSE
UninstallDisplayIcon={app}\procast.exe
; procast holds its exe while running: let Restart Manager close it on upgrade and uninstall.
CloseApplications=force
RestartApplications=no

[Files]
Source: "..\target\release\procast.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
; The Node extension host (extensions and Quick AI); procast finds it next to the exe.
Source: "..\exthost\*"; DestDir: "{app}\exthost"; Excludes: "smoke.mjs"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\procast"; Filename: "{app}\procast.exe"

[Run]
Filename: "{app}\procast.exe"; Description: "Start procast now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM procast.exe"; Flags: runhidden; RunOnceId: "StopProcast"

[Registry]
; procast writes this itself ("Open at login"); remove it with the app.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "procast"; Flags: uninsdeletevalue dontcreatekey
