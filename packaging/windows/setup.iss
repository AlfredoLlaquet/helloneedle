; SPDX-License-Identifier: GPL-3.0-or-later
; Per-user setup follows Crumb Writer's Windows installer, released under the Unlicense.
; https://github.com/AlfredoLlaquet/crumbwriter/tree/ec92c550babbb52d34b7c4872b3fecc4c1b91d48/packaging/windows
#ifndef AppVersion
  #error AppVersion is required
#endif
#ifndef PayloadDir
  #error PayloadDir is required
#endif
#ifndef SetupIcon
  #error SetupIcon is required
#endif
#ifndef OutputFolder
  #error OutputFolder is required
#endif
#ifndef InstallerName
  #error InstallerName is required
#endif

[Setup]
AppId=org.thecrumbs.HelloNeedle
AppName=Hello Needle
AppVersion={#AppVersion}
AppPublisher=Alfredo Llaquet-Alsina
AppPublisherURL=https://github.com/AlfredoLlaquet/helloneedle
AppSupportURL=https://github.com/AlfredoLlaquet/helloneedle/issues
AppUpdatesURL=https://github.com/AlfredoLlaquet/helloneedle/releases
DefaultDirName={localappdata}\Programs\Hello Needle
DefaultGroupName=Hello Needle
DisableProgramGroupPage=yes
AllowNoIcons=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.18362
CloseApplications=yes
RestartApplications=no
WizardStyle=modern
DisableWelcomePage=no
UninstallDisplayIcon={app}\Hello Needle.exe
SetupIconFile={#SetupIcon}
OutputDir={#OutputFolder}
OutputBaseFilename={#InstallerName}
Compression=lzma2
SolidCompression=yes
VersionInfoVersion={#AppVersion}
VersionInfoProductName=Hello Needle
VersionInfoDescription=Hello Needle Setup
VersionInfoCompany=Alfredo Llaquet-Alsina

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\share\doc\helloneedle\LICENSE"; DestName: "HelloNeedle-License.txt"; Flags: dontcopy
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{group}\Hello Needle"; Filename: "{app}\Hello Needle.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\Hello Needle"; Filename: "{app}\Hello Needle.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[InstallDelete]
Type: files; Name: "{userdesktop}\Hello Needle.lnk"; Check: not WizardIsTaskSelected('desktopicon') and WasDesktopShortcutSelected

[Run]
Filename: "{app}\Hello Needle.exe"; Description: "{cm:LaunchProgram,Hello Needle}"; Flags: nowait postinstall skipifsilent

[CustomMessages]
english.ViewLicense=View &License
spanish.ViewLicense=Ver &licencia

[Code]
function WasDesktopShortcutSelected: Boolean;
begin
  Result := GetPreviousData('DesktopShortcut', '0') = '1';
end;

procedure RegisterPreviousData(PreviousDataKey: Integer);
begin
  if WizardIsTaskSelected('desktopicon') then
    SetPreviousData(PreviousDataKey, 'DesktopShortcut', '1')
  else
    SetPreviousData(PreviousDataKey, 'DesktopShortcut', '0');
end;

procedure ViewLicense(Sender: TObject);
var
  ResultCode: Integer;
begin
  ExtractTemporaryFile('HelloNeedle-License.txt');
  if not ShellExec('open', ExpandConstant('{tmp}\HelloNeedle-License.txt'), '', '',
                   SW_SHOWNORMAL, ewNoWait, ResultCode) then
    MsgBox(SysErrorMessage(ResultCode), mbError, MB_OK);
end;

procedure InitializeWizard;
var
  LicenseButton: TNewButton;
begin
  LicenseButton := TNewButton.Create(WizardForm);
  LicenseButton.Parent := WizardForm;
  LicenseButton.Caption := CustomMessage('ViewLicense');
  LicenseButton.SetBounds(ScaleX(12), WizardForm.NextButton.Top,
                          ScaleX(96), WizardForm.NextButton.Height);
  LicenseButton.Anchors := [akLeft, akBottom];
  LicenseButton.OnClick := @ViewLicense;
end;
