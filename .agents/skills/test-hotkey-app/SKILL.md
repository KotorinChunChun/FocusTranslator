---
name: test-hotkey-app
description: >
  Verify FocusTranslator's hotkey/cursor-driven recognition behavior on a real
  Windows desktop (Notepad or a purpose-built mock app as the target under
  test) without ever fighting for or stealing foreground window focus. Use
  this skill whenever you're about to test, verify, screenshot, or debug a
  change to FocusTranslator by launching a helper app and pressing the
  capture key (RCtrl) or preview key (LCtrl) over it, clicking an overlay chip
  button, or setting clipboard content (text/image) for the "コピー中の内容"
  flow — even if the request doesn't explicitly say "foreground" or "focus".
  Also use it whenever the change under test touches paragraph/line-wrap
  reconstruction, UIA path node buttons, or region-display overlays, since
  this skill also encodes the test-content requirements (narrow window,
  wrapped long sentence, multiple paragraphs) needed to actually exercise
  those code paths instead of a trivial one-line test that gives false
  confidence.
---

# FocusTranslator 実機検証 (フォーカス非依存)

## なぜこのスキルが必要か

FocusTranslator の認識トリガーは `WindowFromPoint`(カーソル位置直下のウィンドウ)と
`GetAsyncKeyState`(物理キーの押下状態)だけに依存している。この2つは Win32 の仕様として
**ウィンドウフォーカス/フォアグラウンド状態と無関係**に動く。つまり検証用アプリ
(メモ帳など)を前面化する必要は本来ない。

にもかかわらず「確実に前面化してから操作する」方針を取ると、次の問題にぶつかる:

- `SetForegroundWindow` はOSのセキュリティ機能で意図的にブロックされやすい。
- `AttachThreadInput`+`BringWindowToTop` のような定番の回避策すら、環境によっては
  安定して効かない(このリポジトリを別IDEセッションで開いている場合など、ツール呼び出しの
  合間にフォーカスが奪い返されることが実際にあった)。
- `SendKeys` によるテキスト入力はフォーカス依存のため、フォーカスの奪い合いに負けると
  **無関係な別ウィンドウへテスト文字列を誤注入する事故**につながる(実際に一度起きた:
  ユーザーが操作中だったブラウザの投稿欄にテスト文を送ってしまった)。

したがって「フォーカスを勝ち取ろうとする」のではなく、**そもそもフォーカスに依存しない
手段だけで検証を完結させる**方針に倒す。これが最も安全で、かつ確実に動く。

## 手順

`scripts/test_hotkey_capture.ps1` に全手順を実装済み。まずはこれをそのまま(または
パラメータ調整して)実行する。中身を理解せずに使ってよいが、応用が必要な場合のために
各ステップの意図を以下に示す。

1. **メモ帳を起動して1秒待つ。** 起動直後はウィンドウがまだ生成されていない。
2. **`MainWindowHandle` が立つまでポーリングして実ウィンドウの hwnd を確定する。**
   Windows 11 のモダンメモ帳はホストプロセスと実ウィンドウの pid が分かれることがあり、
   `Start-Process` が返す `Process` オブジェクトの `MainWindowHandle` は起動直後 0 のことが
   多い。`Get-Process -Name notepad` で毎回取り直してポーリングする。
3. **テキスト投入に `SendKeys` を使わない。** 対象の編集コントロールを
   `EnumChildWindows` で子孫から再帰的に探し(モダンメモ帳の実体は `Edit` ではなく
   `RichEditD2DPT` クラス)、そのHWNDへ `SendMessageW` で `WM_SETTEXT` (0x000C) を
   直接送ってテキストを設定する。フォーカス・フォアグラウンドが一切不要で、対象HWND
   限定なので他ウィンドウに漏れる余地がない。
4. **`MoveWindow` の後、必ず `GetWindowRect` で実際の座標を測り直す。** 想定した座標に
   本当に移動したとは限らない(特にモダンアプリはDPIスケーリング等でズレることがある)。
5. **実測した矩形の内側にカーソルを置く。** 想定座標ではなく、手順4で測った値を使う。
6. **キーを押す直前に、カーソル直下が本当に目的のプロセスかを検証する。**
   `WindowFromPoint` でカーソル位置のウィンドウを取得し、`GetWindowThreadProcessId` で
   pid を確認し、起動した対象アプリの pid と一致するかを見る。**これが本当に重要な
   安全確認であり、フォアグラウンド確認ではない。** 一致しなければキー送信を中止する。
7. **確認できて初めてホールドキーを押す。** FocusTranslator の既定キャプチャキーは
   右Ctrl (`0xA3`)。プレビューキー(領域表示専用、実際の翻訳は行わない)は既定 左Ctrl
   (`0xA2`)。`keybd_event` で押下→待機→スクリーンショット→解放(`KEYUP`フラグ`2`)まで行う。
8. **後片付けも対象HWNDへ `PostMessageW(WM_CLOSE)` を送るだけでよい。** フォーカス不要。

**全工程を必ず1回のツール呼び出し(1プロセス)内で完結させること。** 呼び出しを分割すると、
間で前のプロセスが終了するたびにフォーカスが他のウィンドウ(IDE等)へ戻り得るだけでなく、
断片化したスクリプトの数だけユーザーへ実行承認を求めることになり体験を損なう。

**例外はチップボタンのクリックのみ。** チップの座標はオーバーレイのレイアウト(表示内容次第で
変わる)に依存するため、事前にスクリーンショットを見て座標を決めるほかない。この場合だけ
「①クリップボード設定+モック起動+キャプチャキー押下(ピン留め)+スクリーンショット」→
(座標を確認)→「②チップクリック+結果スクリーンショット」の2回に分けてよい。ただし
**その2回それぞれの内部は、可能な限り1つのスクリプトに全ステップを詰め込むこと**
(例えば①をさらに「起動だけ」「配置だけ」「キー押下だけ」の3回に分割しない)。

## テスト内容の要件(段落・折り返し機能を検証する場合)

FocusTranslator の中核機能は複数行テキスト(段落・折り返し行)の認識・結合であり、
短い一行の英文だけをテストしても、段落復元・UIAの `TextUnit_Paragraph` 拡張・
UIAパスノードの子孫テキスト連結といった機能の大部分は実行パスに入らず、見た目上
動いているように見えても実質的に検証できていない。段落・折り返し関連の変更を
検証するときは、必ず次の条件を満たすテスト内容にする(`test_hotkey_capture.ps1` の
既定パラメータは既にこれを満たしている):

- **ウィンドウ幅を狭くする**(目安 500〜600px)。折り返しが実際に発生する状況を作るため。
- **右端で折り返されるほど長い1文**を含める。
- **改行(`\r\n\r\n`)で区切られた複数段落**も含める。段落境界の判定・行間ギャップ推定・
  UIAの段落単位拡張が正しく段落を区切れるかを見るため。

単純な動作確認(色や配置だけ見たい等)であれば短文でもよいが、段落検知・折り返し復元・
UIAパスノード関連の機能検証をするときはこれを省略しない。

## 使い方の例

```powershell
# 既定(RCtrl・段落テスト用テキスト・幅550px)でそのまま実行
pwsh -File .Codex/skills/test-hotkey-app/scripts/test_hotkey_capture.ps1

# プレビューキー(LCtrl)側の領域表示をテストする場合
pwsh -File .Codex/skills/test-hotkey-app/scripts/test_hotkey_capture.ps1 -HoldVk 0xA2

# 短い一行だけで見た目だけ確認したい場合
pwsh -File .Codex/skills/test-hotkey-app/scripts/test_hotkey_capture.ps1 -TestText "short line" -WindowWidth 900
```

スクリプト実行後、`-ScreenshotPath` で指定した(既定はスクリプトと同じ場所の
`test_capture.png`)画像を Read ツールで開いて結果を確認する。

## クリップボードを使うテスト(テキスト/画像・「コピー中の内容」ボタン検証)

`scripts/set_clipboard_content.ps1` を使う。

- **テキスト**: `Set-Clipboard -Value $Text` で十分。特別なアパートメント状態は不要。
- **画像**: `[System.Windows.Forms.Clipboard]::SetImage()` は **STA必須**。pwshは既定でMTAの
  ため、そのままでは呼べない。
  - **やってはいけないこと**: 同一プロセス内で `System.Threading.Thread` を起こし
    `SetApartmentState(STA)` してからPowerShellの ScriptBlock を実行する方法は失敗する
    (`PSInvalidOperationException: There is no Runspace available` — ScriptBlockの実行には
    そのスレッドに紐づいたRunspaceが必要で、素の`Thread`には無い)。
  - **正しいやり方**: **別プロセス**として `pwsh -STA -File <script.ps1>` を呼ぶ。
    `pwsh -STA` はUTF-8既定のため日本語コメントを含むスクリプトでもそのまま動く
    (実機で確認済み)。`powershell.exe -STA`(Windows PowerShell 5.1)は使わないこと —
    既定コードページで.ps1を読むため、日本語コメントが文字化けしてパースエラーになる
    (実際に一度これでハマった: `Missing expression after ','` のようなエラーが、無関係な
    箇所を指して出る)。
  - `set_clipboard_content.ps1` は `-ImageLines` 指定時、自分自身のアパートメント状態を見て
    STAでなければ `pwsh -STA -File $PSCommandPath ...` で自己再実行する(呼び出し側は
    意識しなくてよい)。

```powershell
# テキストをクリップボードに設定
pwsh -File .Codex/skills/test-hotkey-app/scripts/set_clipboard_content.ps1 -Text "sample text"

# 画像(2行の英文を描画したビットマップ)をクリップボードに設定
pwsh -File .Codex/skills/test-hotkey-app/scripts/set_clipboard_content.ps1 -Line1 "Clipboard image OCR test." -Line2 "This picture came from the clipboard."
```

## オーバーレイのチップボタンをクリックするテスト

`scripts/click_overlay_chip.ps1` を使う。キー押下時の安全確認(手順6)と同じ考え方で、
**クリック直前にカーソル直下が本当にオーバーレイ(`FocusTranslatorOverlay`)かを
`WindowFromPoint`+`GetClassNameW`で確認してからクリックする**(一致しなければ中止)。

チップの座標はレイアウト依存で事前に分からないため、まず `test_hotkey_capture.ps1` 等で
ピン留め状態のオーバーレイのスクリーンショットを撮り、Readツールで見て座標を決めてから
このスクリプトに渡す。

```powershell
pwsh -File .Codex/skills/test-hotkey-app/scripts/click_overlay_chip.ps1 -X 498 -Y 307
```

## 設定画面・ログビューアを開いて検証する

オーバーレイ以外の画面(設定画面・ログビューア)を確認したいときは `scripts/open_app_window.ps1`
を使う。フォーカス非依存の原則はここでも同じ: 本体のメインウィンドウ(クラス
`FocusTranslatorMain`, 非表示)へ `WM_COMMAND` を `PostMessage` するだけで開き、
`SetForegroundWindow` は使わない。

- メインウィンドウは `FindWindowW` ではなく `EnumWindows` で探すこと
  (`FindWindowW("FocusTranslatorMain", $null)` はこのウィンドウに対して実測で 0 を
  返すことがあった)。
- `WM_COMMAND`(`0x0111`)の `wParam` は `tray.rs` の `CMD_SETTINGS`(=1) /
  `CMD_LOGVIEWER`(=3) の値。トレイメニューを実際に開いてクリックする必要はない。

```powershell
# 設定画面を開いてスクリーンショット
pwsh -File .Codex/skills/test-hotkey-app/scripts/open_app_window.ps1 -Which Settings

# ログビューアを開いてスクリーンショット
pwsh -File .Codex/skills/test-hotkey-app/scripts/open_app_window.ps1 -Which LogViewer

# 検証後に閉じる
pwsh -File .Codex/skills/test-hotkey-app/scripts/open_app_window.ps1 -Which Settings -Close
```

## 新しい編集コントロールクラスに遭遇したら

`Find-EditControl` 関数は既知のクラス名(`RichEditD2DPT` / `Edit` / `RICHEDIT50W` /
`RichEdit20W`)しか探さない。別の補助アプリ(WordPad、ブラウザの入力欄など)を使う場合や
見つからない場合は、`EnumChildWindows` で子孫の `GetClassNameW` を一通り列挙して実際の
クラス名を特定し、`$candidates` 配列に追記する。
