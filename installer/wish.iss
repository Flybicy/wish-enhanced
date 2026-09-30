; Inno Setup script for Wish — engine, web UI and the bundled niubash environment.
; Built by CI into dist/. Inputs: dist/wish/wish.exe, dist/wish/web/**, dist/wish/niubash/**

#define MyAppName "Wish"
#define MyAppVersion "0.3.3"
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
; The WebView2 C entry point, searched beside the executable first, so the
; desktop window also works on systems without Office/OneDrive/WSL loader copies.
Source: "..\dist\wish\WebView2Loader.dll"; DestDir: "{app}"; Flags: ignoreversion
; Shortcut and window branding.
Source: "..\dist\wish\wish.ico"; DestDir: "{app}"; Flags: ignoreversion
; Bundled runtimes: Python (with the document libraries), Node.js and MinGit,
; so a fresh install can do real agent work with no developer prerequisites.
Source: "..\dist\wish\python\*"; DestDir: "{app}\python"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\dist\wish\node\*"; DestDir: "{app}\node"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\dist\wish\git\*"; DestDir: "{app}\git"; Flags: ignoreversion recursesubdirs createallsubdirs
; Document skills seeded into the data directory the server scans by default.
Source: "..\installer\skills\*"; DestDir: "{userdocs}\Wish\data\skills"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\dist\wish\web\*"; DestDir: "{app}\web"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\dist\wish\niubash\*"; DestDir: "{app}\niubash"; Flags: ignoreversion recursesubdirs createallsubdirs

[Dirs]
Name: "{userdocs}\Wish"; Permissions: users-modify

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\wish.ico"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\wish.ico"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app};{app}\niubash;{app}\python;{app}\node;{app}\git\cmd"; Tasks: pathenv; Check: NeedsPath

[Code]
function NeedsPath(): Boolean;
begin
  Result := Pos(Lowercase(ExpandConstant('{app}')), Lowercase(GetEnv('Path'))) = 0;
end;

[Run]
; Double-click shell mode: the exe self-configures and opens the web interface.
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}\web"
Type: filesandordirs; Name: "{app}\niubash"
Type: filesandordirs; Name: "{app}\python"
Type: filesandordirs; Name: "{app}\node"
Type: filesandordirs; Name: "{app}\git"
