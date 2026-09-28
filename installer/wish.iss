; Inno Setup script for Wish — engine, web UI and the bundled niubash environment.
; Built by CI into dist/. Inputs: dist/wish/wish.exe, dist/wish/web/**, dist/wish/niubash/**

#define MyAppName "Wish"
#define MyAppVersion "0.2.0"
#define MyAppPublisher "wish-enhanced"
#define MyAppExeName "wish.exe"

[Setup]
AppId={{7E1F0B4A9C2D4E3B8F5A6B7C8D9E0F11}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\Wish
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
OutputBaseFilename=wish-setup-{#MyAppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "pathenv"; Description: "Add Wish to PATH (wish.exe and niubash commands)"; Flags: checkedonce

[Files]
Source: "..\dist\wish\wish.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\wish\web\*"; DestDir: "{app}\web"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\dist\wish\niubash\*"; DestDir: "{app}\niubash"; Flags: ignoreversion recursesubdirs createallsubdirs

[Dirs]
Name: "{userdocs}\Wish"; Permissions: users-modify

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app};{app}\niubash"; Tasks: pathenv; Check: NeedsPath

[Code]
function NeedsPath(): Boolean;
begin
  Result := Pos(Lowercase(ExpandConstant('{app}')), Lowercase(GetEnv('Path'))) = 0;
end;

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\{#MyAppExeName}"; Parameters: "--config ""{userdocs}\Wish\config.json"""; Flags: postinstall runhidden; 

[UninstallDelete]
Type: filesandordirs; Name: "{app}\web"
Type: filesandordirs; Name: "{app}\niubash"