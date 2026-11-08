; Inno Setup script for the verbacue desktop app.
; ISCC /DVersion=0.1.0 /DVariant=cpu /DSource=dist /DOutput=. packaging\windows\verbacue.iss
; Source holds verbacue.exe and, for the CUDA build, its DLLs.

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Variant
  #define Variant "cpu"
#endif
#ifndef Source
  #define Source "dist"
#endif
#ifndef Output
  #define Output "."
#endif

[Setup]
AppId={{77E2BBA9-17D2-4F43-9633-E1ECE5E9B1E1}
AppName=verbacue
AppVersion={#Version}
AppVerName=verbacue {#Version} ({#Variant})
AppPublisher=verbacue
AppPublisherURL=https://github.com/ksrichard/verbacue
AppSupportURL=https://github.com/ksrichard/verbacue/issues
DefaultDirName={autopf}\verbacue
DefaultGroupName=verbacue
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE
SetupIconFile=..\icons\verbacue.ico
UninstallDisplayIcon={app}\verbacue.exe
OutputDir={#Output}
OutputBaseFilename=verbacue-{#Version}-windows-x64-{#Variant}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Install for this user only, or for all users with administrator rights.
PrivilegesRequiredOverridesAllowed=dialog

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\verbacue"; Filename: "{app}\verbacue.exe"; Parameters: "gui"
Name: "{autodesktop}\verbacue"; Filename: "{app}\verbacue.exe"; Parameters: "gui"; Tasks: desktopicon

[Run]
Filename: "{app}\verbacue.exe"; Parameters: "gui"; Description: "{cm:LaunchProgram,verbacue}"; Flags: nowait postinstall skipifsilent
