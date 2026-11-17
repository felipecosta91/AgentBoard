; Inno Setup script. Build: iscc /DAppVersion=1.2.3 installer\windows.iss  (expects target\release\agentboard.exe)
#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

[Setup]
AppId={{6E1F5B1B-3B7A-4C55-9F3E-AB0A12D0C0DE}
AppName=AgentBoard
AppVersion={#AppVersion}
DefaultDirName={autopf}\AgentBoard
DefaultGroupName=AgentBoard
OutputDir=..\dist
OutputBaseFilename=AgentBoard-windows-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest
SetupIconFile=..\assets\AppIcon.ico
UninstallDisplayIcon={app}\AgentBoard.exe

[Files]
Source: "..\target\release\agentboard.exe"; DestDir: "{app}"; DestName: "AgentBoard.exe"; Flags: ignoreversion

[Icons]
Name: "{group}\AgentBoard"; Filename: "{app}\AgentBoard.exe"
Name: "{autodesktop}\AgentBoard"; Filename: "{app}\AgentBoard.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"

[Run]
Filename: "{app}\AgentBoard.exe"; Description: "Launch AgentBoard"; Flags: nowait postinstall skipifsilent
