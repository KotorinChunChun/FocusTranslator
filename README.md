# Focus Translator v0.1

右Ctrlキーを押している間だけ、マウスポインタ直下のテキスト1行を認識・翻訳してカーソル近傍にオーバーレイ表示する Windows 11 用タスクトレイ常駐ツール。[FocusTranslator_SPECv0.1.md](FocusTranslator_SPECv0.1.md) に基づく実装。

## ビルド

```
cargo build --release
```

生成物: `target\release\focus-translator.exe`(単一EXE、外部DLL不要)

要件: Rust stable (x86_64-pc-windows-msvc), Windows 11

## 使い方

| 操作 | 動作 |
|---|---|
| **右Ctrl ホールド** | カーソル下の1行を認識・翻訳して表示。離すと消える |
| **Ctrl+Alt+T** | 範囲指定モード。ドラッグ選択した矩形をOCR・翻訳(最初からピン留め) |
| **チップ押下** | OCR/翻訳エンジンを切替えて再処理。押下時点でピン留め |
| **コピー** | 訳文(なければ原文)をクリップボードへ |
| **Esc / 閉じる** | ピン留め表示を閉じる |
| **タスクトレイ** | 設定画面 / 範囲指定翻訳 / 終了 |

## アーキテクチャ

```
src/
├── main.rs      状態機械 (IDLE/RECOGNIZING/SHOWING_HOLD/PINNED)、メッセージループ、
│                100ms GetAsyncKeyState ポーリング、世代番号によるレース制御
├── worker.rs    認識・翻訳ワーカースレッド (UIA優先 → WGC帯OCR、二段階表示)
├── uia.rs       経路A: UI Automation TextPattern による行テキスト取得
├── capture.rs   経路B: Windows.Graphics.Capture (BitBlt不使用)、帯切り出し、PNG化
├── ocr.rs       OCRエンジン群 (Windows.Media.Ocr / 外部HTTP / Gemini統合)
├── translate.rs 翻訳エンジン群 (DeepL / Google / Gemini / ローカル) + キャッシュ
├── overlay.rs   結果オーバーレイ (原文小・訳文大・チップ列、部分ヒットテスト)
├── region.rs    範囲指定モードの選択オーバーレイ
├── settings.rs  設定画面 (ホットキー/エンジン/APIキー[マスク表示]/URL/常駐/ログ/PaddleOCR導入)
├── paddle_install.rs  PaddleOCR(RapidOCR配布ONNX)モデルのSHA256検証付きダウンロード
├── tray.rs      タスクトレイ常駐
├── config.rs    設定永続化 (%APPDATA%\FocusTranslator\config.json)
└── util.rs      DPAPI暗号化、クリップボード、計測ログ
```

## エンジン対応状況

| エンジン | 種別 | 状態 |
|---|---|---|
| Windows.Media.Ocr | OCR (既定) | ✅ 実装済み |
| YomiToku / NDL-OCR | OCR (外部サーバー) | ✅ HTTPクライアント実装済み (`POST /ocr`, `GET /health`) |
| Gemini | OCR+翻訳統合 | ✅ 実装済み (画像→原文+訳文一括) |
| PaddleOCR | OCR | ⏳ モデル導入(設定画面からワンクリックDL+SHA256検証、[paddle_install.rs](src/paddle_install.rs))は実装済み。ONNX Runtime推論は次版 |
| DeepL / Google / Gemini | 翻訳 | ✅ 実装済み (REST) |
| ローカルONNX翻訳 | 翻訳 (既定) | ⏳ モデル配布基盤が未整備のため推論は次版。未導入時はエラー表示し、クラウドエンジンへの切替を案内 |

外部OCRサーバーの想定API: `POST {url}/ocr` (body: `image/png`) → `{"text": "..."}` または `{"results": [{"text": "..."}]}`、`GET {url}/health`。

## プライバシー (SPEC §9)

- 既定構成 (Windows OCR + ローカル翻訳) では外部送信なし。
- クラウド/外部サーバーエンジンの初回利用時に送信種別ごと(テキスト/画像/外部OCRサーバー)の同意ダイアログを表示。`127.0.0.1` へのサーバーURLはローカル送信として扱う。
- 同意なしにホールドモードの既定エンジンが外部送信することはない(未同意時はローカルエンジンへ自動代替)。
- APIキーは DPAPI で暗号化して保存。キー・送信テキスト・画像はログに出さない。設定画面の入力欄も ● でマスク表示。
- 計測ログ(設定で有効化)はステージ別所要時間のみ記録。

## PaddleOCRモデルの導入

設定画面の「PaddleOCR」行で導入状況を確認でき、未導入時は「インストール」ボタンでワンクリック導入できる。
配布元は [RapidAI/RapidOCR](https://github.com/RapidAI/RapidOCR)(ModelScope)公開の PP-OCRv4 mobile 版 ONNX(検出/日本語認識/辞書の3ファイル、計約14MB)。ダウンロード後にSHA256を検証し、一致した場合のみ `%APPDATA%\FocusTranslator\models\paddleocr\` に配置する(不一致時は破棄しエラー表示)。
推論(ONNX Runtime連携)自体は次版対応で、導入済みの状態でPaddleOCRチップを選択すると「推論は未実装です」と表示される。

## 実測値 (開発環境でのスモークテスト)

- 押下 → 原文表示: UIA経路 約3ms / OCR経路 約200〜250ms
- アイドル時メモリ: 約13〜15MB

## SPECからの逸脱・未実装事項

- **ローカルONNX翻訳の推論** (M3): モデル選定・配布(チェックサム検証付きダウンロード)が前提のため次版。エンジン基盤・キャッシュ・フォールバック(クラウド失敗→local バッジ)は実装済み。
- **PaddleOCR** (M0): モデル導入(ダウンロード+チェックサム検証)は実装済み。推論(ONNX Runtime)は次版。
- **行矩形逸脱の監視**: UIAの行矩形ではなくカーソル移動距離(48px)で再認識をトリガする簡易実装。
- **候補展開・解説ボタン** (SPEC §10): 初版UIでは未実装(コピー・閉じる・チップ切替は実装済み)。
- **自動更新・インストーラ** (SPEC §13): 未実装(単一EXE配布)。
- ゲーム / HDR / 保護コンテンツ / 排他フルスクリーン / 縦書きは SPEC §15 のとおり対象外。
