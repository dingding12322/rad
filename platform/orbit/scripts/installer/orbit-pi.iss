; Inno Setup script for Orbit Pi.
;
; Builds a single-file installer, Orbit-Pi-<version>-<arch>-Setup.exe, which
; is both the artifact users download and the payload the in-app updater runs
; (with /SILENT /NORESTART /SP- /DIR=<current install>, see updater.rs). The
; installer is per-user and needs no elevation, so a silent update can replace
; the running install without a UAC prompt.
;
; Compiled by scripts/bundle-windows.ps1, which passes the defines below.
; To build by hand:
;   ISCC.exe /DAppVersion=0.0.2 /DSourceDir=<abs release dir> ^
;            /DOutputDir=<abs dist dir> /DTargetArch=x86_64 /DInnoArch=x64 ^
;            scripts/installer/orbit-pi.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\target\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
; Package filename arch: x86_64 / aarch64 (matches appcast.py's globs).
#ifndef TargetArch
  #define TargetArch "x86_64"
#endif
; Inno arch identifier: x64 / arm64.
#ifndef InnoArch
  #define InnoArch "x64"
#endif

#define AppName "Orbit Pi"
#define AppPublisher "Orbit"
#define AppExeName "orbit-pi.exe"

[Setup]
; A stable AppId keeps upgrades (and the silent self-update) tied to one
; uninstall entry. Never change it.
AppId={{8F1C2E4A-7B3D-4E6F-9A2C-5D8B1E3F7A90}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
; Per-user install under %LocalAppData%\Programs: no UAC, and the updater can
; replace files without elevation.
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename=Orbit-Pi-{#AppVersion}-{#TargetArch}-Setup
SetupIconFile=..\..\assets\icons\icon.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
MinVersion=10.0
ArchitecturesAllowed={#InnoArch}
ArchitecturesInstallIn64BitMode={#InnoArch}
; The app quits itself before launching the installer; force-closing any
; straggler avoids a "file in use" dead end during a silent update.
CloseApplications=force
RestartApplications=no

[Files]
Source: "{#SourceDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Run]
; Runs on silent updates too (no `postinstall`/`skipifsilent`), so the app
; comes back after the updater replaces it.
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait
