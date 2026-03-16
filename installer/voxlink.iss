; Voxlink Installer Script for Inno Setup
; Produces a single .exe installer for Windows distribution
; Bundles VC++ Runtime so end users need ZERO prerequisites

#define MyAppName "Voxlink"
#define MyAppVersion "0.3.1"
#define MyAppPublisher "Voxlink"
#define MyAppURL "https://github.com/voxlink"
#define MyAppExeName "Voxlink.exe"

[Setup]
AppId={{B7A3F2E1-4D5C-6E7F-8A9B-0C1D2E3F4A5B}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\target\installer
OutputBaseFilename=Voxlink-Setup-{#MyAppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
SetupIconFile=..\assets\icon.ico

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "startupentry"; Description: "Launch Voxlink on Windows startup"; GroupDescription: "Other:"; Flags: unchecked

[Files]
; Main application binary (renamed to Voxlink.exe for clean user experience)
Source: "..\target\release\app_desktop.exe"; DestDir: "{app}"; DestName: "Voxlink.exe"; Flags: ignoreversion

; Include the signaling server too so users can host
Source: "..\target\release\signaling_server.exe"; DestDir: "{app}"; DestName: "Voxlink-Server.exe"; Flags: ignoreversion

; VC++ Runtime redistributable — installed silently if needed
Source: "..\installer\redist\vc_redist.x64.exe"; DestDir: "{tmp}"; Flags: deleteafterinstall; Check: VCRedistNeeded

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\{#MyAppName} Server"; Filename: "{app}\Voxlink-Server.exe"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Registry]
; Optional startup entry
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#MyAppName}"; ValueData: """{app}\{#MyAppExeName}"""; Flags: uninsdeletevalue; Tasks: startupentry

[Run]
; Install VC++ Runtime silently before launching the app (only if needed)
Filename: "{tmp}\vc_redist.x64.exe"; Parameters: "/install /quiet /norestart"; StatusMsg: "Installing Visual C++ Runtime..."; Flags: waituntilterminated; Check: VCRedistNeeded

; Launch app after install
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Clean up config directory on uninstall
Type: filesandordirs; Name: "{localappdata}\com.voxlink.Voxlink"

[Code]
// Check if VC++ 2015-2022 Redistributable (x64) is already installed
// by looking for the runtime DLL version. If vcruntime140.dll exists
// and is recent enough, skip the install.
function VCRedistNeeded: Boolean;
var
  Version: String;
begin
  // Check registry for VC++ 2015-2022 Redist
  Result := True;
  if RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Version', Version) then
  begin
    // v14.x means VC++ 2015-2022 is installed
    if (Length(Version) > 0) then
      Result := False;
  end;
  if RegQueryStringValue(HKLM, 'SOFTWARE\WOW6432Node\Microsoft\VisualStudio\14.0\VC\Runtimes\x64', 'Version', Version) then
  begin
    if (Length(Version) > 0) then
      Result := False;
  end;
end;

// Add Windows Firewall exception for the server on first install
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep = ssPostInstall then
  begin
    // Add firewall rule for the signaling server (TCP 9090)
    // This prevents the Windows Firewall popup when users host a server
    Exec('netsh', 'advfirewall firewall add rule name="Voxlink Server" dir=in action=allow program="' + ExpandConstant('{app}\Voxlink-Server.exe') + '" enable=yes profile=private,public protocol=tcp localport=9090', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  end;
end;

// Remove firewall rule on uninstall
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ResultCode: Integer;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    Exec('netsh', 'advfirewall firewall delete rule name="Voxlink Server"', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  end;
end;
