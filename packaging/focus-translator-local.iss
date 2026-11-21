#ifndef Payload
  #error Payload is required
#endif
#ifndef OutputDirectory
  #error OutputDirectory is required
#endif
[Setup]
AppId=KotorinToys.FocusTranslator
AppName=FocusTranslator
AppVersion=0.5.7
DefaultDirName={localappdata}\Programs\FocusTranslator
PrivilegesRequired=lowest
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0.22000
OutputDir={#OutputDirectory}
OutputBaseFilename=FocusTranslator-0.5.7-setup
Compression=lzma2/fast
SolidCompression=yes
CloseApplications=no
RestartApplications=no
DisableProgramGroupPage=yes
UninstallDisplayIcon={app}\FocusTranslator.exe,0
SetupIconFile=..\FocusTranslator_icon_32x32.ico
WizardStyle=modern
[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"
[Files]
Source: "{#Payload}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{userprograms}\FocusTranslator"; Filename: "{app}\FocusTranslator.exe"
