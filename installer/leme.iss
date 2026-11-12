; Inno Setup script for leme. Build with:
;   iscc /DAppVersion=1.0.0 /DSourceDir=..\target\release installer\leme.iss
; Installs per user by default (no admin prompt); the .pdf association
; only adds Leme to "Open with", since Windows lets the user pick defaults.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif

[Setup]
AppId={{6B0F5C2E-8E3A-4D3B-9C61-2F1A7D4E9B10}
AppName=Leme
AppVersion={#AppVersion}
AppVerName=Leme {#AppVersion}
AppPublisher=Diego Rezende
AppPublisherURL=https://github.com/diegorezm/leme
AppSupportURL=https://github.com/diegorezm/leme/issues
DefaultDirName={autopf}\Leme
DefaultGroupName=Leme
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=leme-{#AppVersion}-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\leme.exe
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes

[Tasks]
Name: "pdfassoc"; Description: "Add Leme to ""Open with"" for PDF files"; GroupDescription: "File types:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\leme.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

; Upgrading from dpdf, the former name: remove what leme.exe replaces.
[InstallDelete]
Type: files; Name: "{app}\dpdf.exe"
Type: files; Name: "{autoprograms}\dpdf.lnk"
Type: files; Name: "{autodesktop}\dpdf.lnk"

[Icons]
Name: "{autoprograms}\Leme"; Filename: "{app}\leme.exe"
Name: "{autodesktop}\Leme"; Filename: "{app}\leme.exe"; Tasks: desktopicon

[Registry]
; A ProgId for PDFs, offered in "Open with" and Default apps.
Root: HKA; Subkey: "Software\Classes\leme.pdf"; ValueType: string; ValueData: "PDF document"; Flags: uninsdeletekey; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\leme.pdf\DefaultIcon"; ValueType: string; ValueData: "{app}\leme.exe,0"; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\leme.pdf\shell\open\command"; ValueType: string; ValueData: """{app}\leme.exe"" ""%1"""; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "leme.pdf"; ValueData: ""; Flags: uninsdeletevalue; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Applications\leme.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Leme"; Flags: uninsdeletekey; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Applications\leme.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Applications\leme.exe\shell\open\command"; ValueType: string; ValueData: """{app}\leme.exe"" ""%1"""; Tasks: pdfassoc

Root: HKA; Subkey: "Software\Classes\dpdf.pdf"; ValueType: none; Flags: deletekey
Root: HKA; Subkey: "Software\Classes\Applications\dpdf.exe"; ValueType: none; Flags: deletekey
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: none; ValueName: "dpdf.pdf"; Flags: deletevalue

[Run]
Filename: "{app}\leme.exe"; Description: "Launch Leme"; Flags: nowait postinstall skipifsilent
; Silent installs are Leme updating itself: open it again.
Filename: "{app}\leme.exe"; Flags: nowait skipifnotsilent
