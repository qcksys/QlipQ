; Inno Setup script for the qlipq Windows installer.
;
; Compiled in CI (qlipq-desktop-release.yml) from apps/desktop/dist, which holds qlipq.exe, the bundled
; LGPL FFmpeg DLLs, and FFMPEG-LICENSE.txt. The version is passed on the command line:
;   ISCC /DMyAppVersion=X.Y.Z installer\qlipq.iss
#ifndef MyAppVersion
  #define MyAppVersion "0.0.0"
#endif
#define MyAppName "qlipq"
#define MyAppPublisher "qcksys"
#define MyAppExeName "qlipq.exe"
#define MyAppURL "https://qlipq.com"

[Setup]
; AppId uniquely identifies the app for upgrades/uninstall — keep it stable across versions.
AppId={{36C074CA-3CDE-4FD8-A823-778FF2B56CCB}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\{#MyAppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Paths below are resolved relative to SourceDir (apps/desktop), one level up from this script.
SourceDir=..
OutputDir=installer\Output
OutputBaseFilename=qlipq-setup-x64
LicenseFile=dist\FFMPEG-LICENSE.txt

[Files]
Source: "dist\*"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent
