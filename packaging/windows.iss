; DeskSync Windows 安装器（Inno Setup）
; 由 CI 调用：ISCC packaging/windows.iss /DMyAppVersion=0.2.0

#define MyAppName "DeskSync"
#define MyAppPublisher "KiwiTK"
#define MyAppURL "https://github.com/KiwiTK/DeskSync"
#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

[Setup]
AppId={{8F1B2C4D-6E7A-4B93-9D21-5A0C7E3F1B84}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
LicenseFile=..\LICENSE
OutputDir=..\dist
OutputBaseFilename=DeskSync-{#MyAppVersion}-Windows-x64-Setup
SetupIconFile=..\assets\DeskSync.ico
UninstallDisplayIcon={app}\desksync.exe
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequiredOverridesAllowed=dialog

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startup"; Description: "开机自动启动（登录时启动 DeskSync）"; GroupDescription: "附加选项:";
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加选项:"; Flags: unchecked

[Files]
Source: "..\target\x86_64-pc-windows-msvc\release\desksync.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\desksync.exe"
Name: "{group}\卸载 {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\desksync.exe"; Tasks: desktopicon

[Registry]
; 开机自启：写 Run 键而不是放服务，用户可在任务管理器里自行开关
Root: HKCU; Subkey: "SOFTWARE\Microsoft\Windows\CurrentVersion\Run"; \
    ValueType: string; ValueName: "DeskSync"; ValueData: """{app}\desksync.exe"""; \
    Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\desksync.exe"; Description: "现在启动 DeskSync"; \
    Flags: nowait postinstall skipifsilent
