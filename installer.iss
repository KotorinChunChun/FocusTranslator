[Setup]
; AppId は旧バージョン (AppName=Focus Translator 時代) からの上書き更新を維持するため固定
AppId=Focus Translator
AppName=なにこれ？（Focus Translator）
AppVersion=0.4.8
AppPublisher=Focus Translator Team
; インストール先はパス互換のため内部名のまま
DefaultDirName={autopf}\Focus Translator
DefaultGroupName=なにこれ？（Focus Translator）
OutputDir=Output
OutputBaseFilename=focus-translator-setup
Compression=lzma
SolidCompression=yes
ArchitecturesInstallIn64BitMode=x64
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\focus-translator.exe

[Files]
Source: "target\release\focus-translator.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "target\release\DirectML.dll"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\なにこれ？（Focus Translator）"; Filename: "{app}\focus-translator.exe"
Name: "{group}\{cm:UninstallProgram,なにこれ？（Focus Translator）}"; Filename: "{uninstallexe}"
; スタートアップに登録する常駐ソフトとするため、自動起動のショートカットを作成
Name: "{userstartup}\なにこれ？（Focus Translator）"; Filename: "{app}\focus-translator.exe"

[Run]
Filename: "{app}\focus-translator.exe"; Description: "なにこれ？（Focus Translator）を起動する"; Flags: nowait postinstall skipifsilent
