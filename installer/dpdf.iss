; Inno Setup script for dpdf. Build with:
;   iscc /DAppVersion=1.0.0 /DSourceDir=..\target\release installer\dpdf.iss
; Installs per user by default (no admin prompt); the .pdf association
; only adds dpdf to "Open with", since Windows lets the user pick defaults.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif

[Setup]
AppId={{6B0F5C2E-8E3A-4D3B-9C61-2F1A7D4E9B10}
AppName=dpdf
AppVersion={#AppVersion}
AppVerName=dpdf {#AppVersion}
AppPublisher=Diego Rezende
AppPublisherURL=https://github.com/diegorezm/dpdf
AppSupportURL=https://github.com/diegorezm/dpdf/issues
DefaultDirName={autopf}\dpdf
DefaultGroupName=dpdf
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=dpdf-{#AppVersion}-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\dpdf.exe
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes

[Tasks]
Name: "pdfassoc"; Description: "Add dpdf to ""Open with"" for PDF files"; GroupDescription: "File types:"
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\dpdf.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\dpdf"; Filename: "{app}\dpdf.exe"
Name: "{autodesktop}\dpdf"; Filename: "{app}\dpdf.exe"; Tasks: desktopicon

[Registry]
; A ProgId for PDFs, offered in "Open with" and Default apps.
Root: HKA; Subkey: "Software\Classes\dpdf.pdf"; ValueType: string; ValueData: "PDF document"; Flags: uninsdeletekey; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\dpdf.pdf\DefaultIcon"; ValueType: string; ValueData: "{app}\dpdf.exe,0"; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\dpdf.pdf\shell\open\command"; ValueType: string; ValueData: """{app}\dpdf.exe"" ""%1"""; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "dpdf.pdf"; ValueData: ""; Flags: uninsdeletevalue; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Applications\dpdf.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "dpdf"; Flags: uninsdeletekey; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Applications\dpdf.exe\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""; Tasks: pdfassoc
Root: HKA; Subkey: "Software\Classes\Applications\dpdf.exe\shell\open\command"; ValueType: string; ValueData: """{app}\dpdf.exe"" ""%1"""; Tasks: pdfassoc

[Run]
Filename: "{app}\dpdf.exe"; Description: "Launch dpdf"; Flags: nowait postinstall skipifsilent
