; Inno Setup script for Wield.
; Build: iscc /DAppVersion=0.10.0 installer\wield.iss   (after `cargo build --release`)
;
; Installs per user by default (no administrator prompt, %LOCALAPPDATA%\Programs\Wield);
; the first page lets you install for all users into Program Files instead.
; Silent updates: wield-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS [/NOLAUNCH]

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Keep this AppId stable: the updater uses it to detect installed copies (src/update.rs).
AppId={{8E5B9C2A-6F4D-4A8B-9E2C-57D1A3B0C4F6}
AppName=Wield
AppVersion={#AppVersion}
AppVerName=Wield {#AppVersion}
AppPublisher=Bitangsha Ray
AppPublisherURL=https://github.com/plaintextlab/wield
AppSupportURL=https://github.com/plaintextlab/wield/issues
AppUpdatesURL=https://github.com/plaintextlab/wield/releases
AppCopyright=Copyright (c) 2026 Bitangsha Ray
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\Wield
DefaultGroupName=Wield
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir=..\dist
OutputBaseFilename=wield-v{#AppVersion}-setup
SetupIconFile=wield.ico
UninstallDisplayIcon={app}\wield.exe
UninstallDisplayName=Wield
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no
LicenseFile=..\LICENSE

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "contextmenu"; Description: "Add ""Scan with Wield"" to Explorer's right-click menu"; GroupDescription: "Windows integration:"; Flags: unchecked

[Files]
Source: "..\target\release\wield.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Wield"; Filename: "{app}\wield.exe"; Comment: "Windows toolkit"
Name: "{autodesktop}\Wield"; Filename: "{app}\wield.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Classes\*\shell\WieldScan"; ValueType: string; ValueName: ""; ValueData: "Scan with Wield"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\*\shell\WieldScan"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\wield.exe"",0"; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\*\shell\WieldScan\command"; ValueType: string; ValueName: ""; ValueData: """{app}\wield.exe"" --scan ""%1"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\WieldScan"; ValueType: string; ValueName: ""; ValueData: "Scan with Wield"; Tasks: contextmenu; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\WieldScan"; ValueType: string; ValueName: "Icon"; ValueData: """{app}\wield.exe"",0"; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\WieldScan\command"; ValueType: string; ValueName: ""; ValueData: """{app}\wield.exe"" --scan ""%1"""; Tasks: contextmenu

[Run]
; Interactive install: optional "Launch Wield" checkbox on the last page.
Filename: "{app}\wield.exe"; Description: "{cm:LaunchProgram,Wield}"; Flags: nowait postinstall skipifsilent
; Silent install (used by the in-app updater): reopen Wield unless /NOLAUNCH was passed.
Filename: "{app}\wield.exe"; Flags: nowait runasoriginaluser; Check: ShouldRelaunch

[Code]
function ShouldRelaunch: Boolean;
begin
  Result := WizardSilent and (Pos('/NOLAUNCH', UpperCase(GetCmdTail)) = 0);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  // The app can add the Explorer menu entry itself (Settings), so always remove it on uninstall.
  if CurUninstallStep = usPostUninstall then
  begin
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\*\shell\WieldScan');
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\Directory\shell\WieldScan');
  end;
end;
