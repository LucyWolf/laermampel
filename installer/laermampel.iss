; Installer für die Lärmampel (Inno Setup 6).
; Installiert pro Benutzer ohne Adminrechte, damit sich das Programm selbst aktualisieren kann.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{6F3C2A9E-4B1D-4E7A-9C52-8D0B7A1E5F34}
AppName=Lärmampel
AppVersion={#AppVersion}
AppVerName=Lärmampel {#AppVersion}
AppPublisher=LucyWolf
AppPublisherURL=https://github.com/LucyWolf/laermampel
AppSupportURL=https://github.com/LucyWolf/laermampel/issues
DefaultDirName={localappdata}\Programs\Laermampel
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=Laermampel-Setup-{#AppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=Lärmampel
UninstallDisplayIcon={app}\laermampel.exe
CloseApplications=force
RestartApplications=no

[Languages]
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "autostart"; Description: "Mit Windows starten"

[Files]
Source: "..\target\release\laermampel.exe"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; Derselbe Wert, den auch der Schalter in den Einstellungen setzt.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Laermampel"; ValueData: """{app}\laermampel.exe"""; Tasks: autostart

[Run]
Filename: "{app}\laermampel.exe"; Description: "Lärmampel jetzt starten"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM laermampel.exe"; Flags: runhidden; RunOnceId: "LaermampelBeenden"

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // Autostart entfernen, auch wenn er später in den Einstellungen eingeschaltet wurde.
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Laermampel');
  end;
end;
