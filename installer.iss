[Setup]
; AppId は旧バージョン (AppName=Focus Translator 時代) からの上書き更新を維持するため固定
AppId=Focus Translator
AppName=なにこれ？（Focus Translator）
AppVersion=0.5.4
AppPublisher=Kotorichun
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

; アンインストール時のUIを日本語にする (SPECv0.5.4 §19)
[Languages]
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

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

[UninstallRun]
; アンインストール開始前に、実行中の本体を終了しLLMサーバーを停止する (SPECv0.5.4 §19)
Filename: "{app}\focus-translator.exe"; Parameters: "--uninstall-cleanup"; Flags: waituntilterminated runhidden; RunOnceId: "CleanupRunningApp"

[Code]
// アンインストール時、当アプリが %APPDATA%\FocusTranslator に作成したデータ
// (ログ・翻訳モデル・LLMサーバー/モデル・設定) を一括削除するか利用者に確認する
// (SPECv0.5.4 §19)。
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    DataDir := ExpandConstant('{userappdata}\FocusTranslator');
    if DirExists(DataDir) then
    begin
      if MsgBox('当アプリが作成したデータ（ログ・翻訳モデル・LLMサーバーとモデル・設定）を'
        + #13#10 + 'すべて削除しますか？' + #13#10 + #13#10
        + '「いいえ」を選ぶと ' + DataDir + ' 配下のデータは残ります。',
        mbConfirmation, MB_YESNO) = IDYES then
      begin
        DelTree(DataDir, True, True, True);
      end;
    end;
  end;
end;
