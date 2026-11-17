; Inno Setup script for the Dryer installer.
; Build it with tools\build-installer.ps1 (which passes AppVersion and ExePath).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef ExePath
  #define ExePath "..\target\release\dryer.exe"
#endif

[Setup]
; A fixed id, so a newer installer upgrades the installed copy instead of adding a second one.
AppId={{F547F428-711A-47BC-BA43-B6629171EA54}
AppName=Dryer
AppVersion={#AppVersion}
AppVerName=Dryer {#AppVersion}
AppPublisherURL=https://github.com/avDec25/dryer
AppSupportURL=https://github.com/avDec25/dryer/issues
AppUpdatesURL=https://github.com/avDec25/dryer/releases
VersionInfoVersion={#AppVersion}
; Installed for the current user only: no administrator prompt, and it matches where
; Dryer keeps its history and start-at-login entry (both per user).
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\Dryer
DefaultGroupName=Dryer
DisableProgramGroupPage=yes
OutputDir=..\dist
OutputBaseFilename=DryerSetup-{#AppVersion}
SetupIconFile=..\icon.ico
UninstallDisplayIcon={app}\dryer.exe
Compression=lzma2/ultra
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Dryer allows one instance and holds this mutex, so Setup can notice it is running
; and offer to close it before replacing the program (it saves its history on close).
AppMutex=Local\DryerSingleInstance
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startup"; Description: "Start Dryer when I sign in to Windows"; GroupDescription: "Options:"; Flags: unchecked
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Options:"; Flags: unchecked

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\Readme.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Dryer"; Filename: "{app}\dryer.exe"
Name: "{group}\Uninstall Dryer"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Dryer"; Filename: "{app}\dryer.exe"; Tasks: desktopicon

[Registry]
; Same entry as the tray menu's "Start with Windows" toggle.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Dryer"; ValueData: """{app}\dryer.exe"""; Tasks: startup

[Run]
Filename: "{app}\dryer.exe"; Description: "Start Dryer now"; Flags: nowait postinstall skipifsilent

[Code]
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    // Remove the start-at-login entry, whether the installer or the tray toggle made it.
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'Dryer');
    // The saved clipboard history is the user's data: keep it unless they say otherwise
    // (and always keep it in a silent uninstall).
    if not UninstallSilent then
      if MsgBox('Also delete your saved clipboard history and settings?', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(ExpandConstant('{localappdata}\Dryer'), True, True, True);
  end;
end;