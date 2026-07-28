---
name: diagnose-with-mock
description: >
  Build a minimal, throwaway Win32/UIA reproduction app (a cargo example
  under examples/) to confirm the root cause of a suspected FocusTranslator
  bug on a real desktop, before writing the fix. Use this when a bug report
  describes behavior that "should" work according to the code but doesn't —
  especially UI Automation quirks (element identification, text/selection
  retrieval, window ownership/title resolution) that can't be settled by
  reading source alone and depend on real OS/control behavior. Differs from
  test-hotkey-app (which verifies already-implemented, known behavior): this
  skill is for diagnosing an *unknown* root cause via a constructed
  experiment first, then handing off to test-hotkey-app-style scripts to
  actually drive the mock without stealing focus.
---

# 診断用モックアプリでの原因究明

## なぜこのスキルが必要か

UIA/Win32の挙動は、実装コードを読むだけでは断定できないことがある。たとえば
「標準Win32 EDITコントロールはCOM UIAでもTextPatternに非対応」「メニューポップアップは
タイトルを持たない」といった仕様は、コード上のロジックが正しく見えても実機で確認しない
限り確信が持てない。

かといって、本体アプリ内に一時的な診断コードを埋め込んで都度リビルド・手動操作するのは
非効率で、痕跡も残りやすい。**独立した最小の再現アプリ(cargo example)を作り、本体には
恒久的な(デバッグモード限定の)診断ログだけを足す**方式のほうが、繰り返し実験でき、
本番コードへの影響も最小限に抑えられる。

## 手順

1. **仮説を先に明文化する。** 「ElementFromPointの要素特定自体は合っているが、選択を
   取得する手段そのものが存在しないのでは」のように、何を確認すれば白黒つくかを
   具体的に書き出してから着手する。仮説なしに実験を始めない。
2. **仮説を再現できる最小のWin32モックアプリを `examples/` に作る。** 本体の `src/*.rs`
   は変更しない。既存の `examples/uia_mock.rs` (単一行/複数行EDIT・編集可能コンボ
   ボックス・メニューバーを持つ) を流用・拡張できないか先に確認する。まったく別種の
   UIが必要なとき(WordPad、ブラウザ入力欄等)のみ新規exampleを作る。
3. **本体側に、疑わしい処理の直後に診断ログを1行足す。** `cfg.debug_mode` でガードし、
   「要素の特定は合っているか」と「値は実際に取れているか」の**両方**を1行に出す
   (例: `uia-probe: node={:?} selected={:?} ...`)。片方だけだと、要素特定の誤りと
   値取得の失敗を切り分けられない。
4. **`test-hotkey-app` スキルの手順でモックへキー送信/クリックする。** フォーカスを
   奪わない・`SendKeys`を使わない等の原則はそのまま適用する。
5. **`app.log`(`%APPDATA%\FocusTranslator\app.log`、テスト時は`FOCUSTRANSLATOR_DATA_DIR`
   で隔離)を読んで、仮説が正しかったか判定する。** 実行前の行数を記録しておき、
   実行後に増えた分だけを見る(差分確認)。
6. **原因が確定したら、直接実装に進まずSPECファイル(`dev/end/vX.X.X ...md`)に
   原因と修正方針を書き出す。** 原因未確定のまま「たぶんこれだろう」で実装しない
   (このリポジトリの開発フロー全般の方針でもある)。
7. **診断ログは本体のバグ再発時にも役立つなら残す**(`debug_mode`限定なので通常運用への
   影響はない)。今回限りの実験用なら削除する。モックアプリ(`examples/`)自体は
   「今後のテスト用ツールの1つ」として残すのが基本(使い捨てでいい理由がない限り)。

## このリポジトリでの適用例

- **選択中文字列が検出できない問題**: `examples/uia_mock.rs` に単一行/複数行EDITと
  編集可能コンボボックスを用意し、`EM_SETSEL`でフォーカス非依存に選択を作ってから
  `recognize_cycle`の診断ログ(`uia-probe: node=... selected=...`)を確認 →
  要素特定(`node`)は正しいが`selected`が常に`None` → 標準Win32 EDITはTextPattern
  非対応と確定 → `IUIAutomationElement::CurrentNativeWindowHandle()`経由の
  `EM_GETSEL`フォールバックを実装。
- **メニュー項目キャプチャで親アプリが取れない問題**: `uia_mock.rs`にメニューバーを
  追加し、メニュー項目上でキャプチャして診断ログ(`window_diag`: target/owner/
  foregroundそれぞれのclass/exe/title)を確認 → メニューウィンドウ(`#32768`)は
  実行ファイル名は取れるがタイトルを持たない、`GW_OWNER`で辿れるオーナーは持つ、
  と確定 → タイトルが空のときオーナーへフォールバックする処理を実装。

## 関連スキル

- モックの起動・キー送信・クリック・クリップボード設定はフォーカス非依存で行う:
  `test-hotkey-app` スキル(`scripts/`配下の各ps1をそのまま使える)。
- 原因確定・修正実装後のドキュメント同期: `finish-feature` スキル。
- 実装後のビルド/テスト/clippy確認: `verify-build` スキル。
