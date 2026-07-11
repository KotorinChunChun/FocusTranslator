---
name: test-hotkey-app
description: >
  Verify FocusTranslator's hotkey/cursor-driven recognition behavior on a real
  Windows desktop (Notepad as the target under test) without ever fighting for
  or stealing foreground window focus. Use this skill whenever you're about to
  test, verify, screenshot, or debug a change to FocusTranslator by launching
  Notepad (or any other helper app) and pressing the capture key (RCtrl) or
  preview key (LCtrl) over it — even if the request doesn't explicitly say
  "foreground" or "focus". Also use it whenever the change under test touches
  paragraph/line-wrap reconstruction, UIA path node buttons, or region-display
  overlays, since this skill also encodes the test-content requirements
  (narrow window, wrapped long sentence, multiple paragraphs) needed to
  actually exercise those code paths instead of a trivial one-line test that
  gives false confidence.
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
間で前のプロセスが終了するたびにフォーカスが他のウィンドウ(IDE等)へ戻り得る。

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
pwsh -File .claude/skills/test-hotkey-app/scripts/test_hotkey_capture.ps1

# プレビューキー(LCtrl)側の領域表示をテストする場合
pwsh -File .claude/skills/test-hotkey-app/scripts/test_hotkey_capture.ps1 -HoldVk 0xA2

# 短い一行だけで見た目だけ確認したい場合
pwsh -File .claude/skills/test-hotkey-app/scripts/test_hotkey_capture.ps1 -TestText "short line" -WindowWidth 900
```

スクリプト実行後、`-ScreenshotPath` で指定した(既定はスクリプトと同じ場所の
`test_capture.png`)画像を Read ツールで開いて結果を確認する。

## 新しい編集コントロールクラスに遭遇したら

`Find-EditControl` 関数は既知のクラス名(`RichEditD2DPT` / `Edit` / `RICHEDIT50W` /
`RichEdit20W`)しか探さない。別の補助アプリ(WordPad、ブラウザの入力欄など)を使う場合や
見つからない場合は、`EnumChildWindows` で子孫の `GetClassNameW` を一通り列挙して実際の
クラス名を特定し、`$candidates` 配列に追記する。
