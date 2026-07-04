# Focus Translator v0.1

右Ctrlキーを押している間だけ、マウスポインタ直下のテキスト1行を認識・翻訳してカーソル近傍にオーバーレイ表示する Windows 11 用タスクトレイ常駐ツール。[FocusTranslator_SPECv0.1.md](FocusTranslator_SPECv0.1.md) に基づく実装。

## ビルド

```
cargo build --release
```

生成物: `target\release\focus-translator.exe`。ONNX Runtimeは静的リンクのため通常動作(CPU推論)に外部DLLは不要。`DirectML.dll` は同梱されるが遅延ロードのDirectML実行プロバイダ用で、未使用のため無くても起動・動作する。

要件: Rust stable (x86_64-pc-windows-msvc), Windows 11

## 設定・モデルの保存先

既定では `%APPDATA%\FocusTranslator\` (config.json, models\, ログ) に保存される。
開発・動作確認時に実際のユーザー設定(APIキー・サーバーURL・導入済みモデル)を壊さないよう、
環境変数 `FOCUSTRANSLATOR_DATA_DIR` を設定すると保存先を任意のディレクトリに差し替えられる。

```
set FOCUSTRANSLATOR_DATA_DIR=C:\path\to\scratch\dir
focus-translator.exe
```

動作確認やテストを行う場合は、この環境変数で隔離したディレクトリを使い、
`%APPDATA%\FocusTranslator\` 本体には触れないこと。

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
├── settings.rs  設定画面 (ホットキー/エンジン/APIキー[マスク表示+取得ページ]/URL/常駐/ログ/モデル導入)
├── paddle_install.rs         PaddleOCR(RapidOCR配布ONNX)モデルのSHA256検証付きダウンロード
├── onnx_translate_install.rs ローカルONNX翻訳(opus-mt ja⇄en)モデルのSHA256検証付きダウンロード
├── onnx_translate.rs         ローカルONNX翻訳の推論本体 (ort + tokenizers、貪欲法デコード)
├── logdb.rs     実行ログのSQLite記録 (認識/翻訳ログ, rusqlite bundled, WAL)
├── logviewer.rs ログビューア (3段ドリルダウン + 画像小表示, ListView)
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
| DeepL / Google Trans / Gemini | 翻訳 | ✅ 実装済み (REST)。設定画面に各APIキー取得ページを開くボタンあり |
| ローカルONNX翻訳 | 翻訳 (既定) | ✅ 実装済み。opus-mt-ja-en / opus-mt-en-jap (ONNX量子化, [onnx_translate.rs](src/onnx_translate.rs)) による貪欲法(greedy)デコード。KVキャッシュ未使用の簡易実装 |

外部OCRサーバーの想定API: `POST {url}/ocr` (body: `image/png`) → `{"text": "..."}` または `{"results": [{"text": "..."}]}`、`GET {url}/health`。

## プライバシー (SPEC §9)

- 既定構成 (Windows OCR + ローカル翻訳) では外部送信なし。
- クラウド/外部サーバーエンジンの初回利用時に送信種別ごと(テキスト/画像/外部OCRサーバー)の同意ダイアログを表示。`127.0.0.1` へのサーバーURLはローカル送信として扱う。
- 同意なしにホールドモードの既定エンジンが外部送信することはない(未同意時はローカルエンジンへ自動代替)。
- APIキーは DPAPI で暗号化して保存。キー・送信テキスト・画像はログに出さない。設定画面の入力欄も ● でマスク表示。
- 計測ログ(設定で有効化)はステージ別所要時間のみ記録。

## モデルの導入(ワンクリックインストール)

設定画面の「PaddleOCR」「ローカルONNX翻訳」の各行で導入状況を確認でき、未導入時は「インストール」ボタンでワンクリック導入できる。いずれもダウンロード後にSHA256を検証し、一致した場合のみ配置する(不一致時は破棄しエラー表示)。

- **PaddleOCR**: 配布元は [RapidAI/RapidOCR](https://github.com/RapidAI/RapidOCR)(ModelScope)公開の PP-OCRv4 mobile 版 ONNX(検出/日本語認識/辞書の3ファイル、計約14MB)。`%APPDATA%\FocusTranslator\models\paddleocr\` に配置。OCRの推論(ONNX Runtime連携)は次版対応で、導入済みの状態でPaddleOCRチップを選択すると「推論は未実装です」と表示される。
- **ローカルONNX翻訳**: 配布元は [Xenova/opus-mt-ja-en](https://huggingface.co/Xenova/opus-mt-ja-en) / [Xenova/opus-mt-en-jap](https://huggingface.co/Xenova/opus-mt-en-jap)(Helsinki-NLP OPUS-MTのONNX量子化版)。日→英・英→日それぞれのencoder/decoder/tokenizer、計6ファイル・約200MB。`%APPDATA%\FocusTranslator\models\onnx_translate\` に配置。導入後は実際に推論が動作する(下記「ローカルONNX翻訳の実装詳細」参照)。

### ローカルONNX翻訳の実装詳細

- `ort` クレート (ONNX Runtime 2.x, 静的リンク) + `tokenizers` クレートで推論する。
- デコードは貪欲法(greedy, ビームサーチではない)。KVキャッシュは使用せず、生成ステップごとにデコーダ入力列全体を再計算する簡易実装(系列長に対しO(n²)だが実装がシンプルで確実)。
- HFのgeneration_configにある `bad_words_ids`(自身の開始/パディングトークンの生成禁止)相当のロジックを実装している。これを入れないと一部の組み合わせで空文字列や意味不明な出力になることを検証で確認した。
- Xenova配布のtokenizer.jsonはPrecompiledノーマライザのcharsmapを含まないため、読込時に当該ノーマライザを無効化している(既知の制約。多くの一般的な文では実用上問題ない)。
- **既知の品質限界**: `opus-mt-en-jap` (英→日) は Helsinki-NLP OPUS-MT の中でも品質が低いモデルとして知られており、実際に検証しても翻訳が不自然になるケースが多い(例: "Thank you very much." → "あなたは多くの人を驚かせた。"のような的外れな訳)。日→英 (`opus-mt-ja-en`) は比較的良好。より高品質な翻訳が必要な場合はクラウドエンジン(DeepL/Google/Gemini)への切替を推奨する。

## APIキーの取得ページ

設定画面の各APIキー入力欄の右側にある「取得ページ」ボタンから、既定ブラウザで発行ページを開ける。

- DeepL: https://www.deepl.com/en/your-account/keys
- Google Trans (Google Cloud Translation API): https://console.cloud.google.com/apis/credentials
- Gemini: https://aistudio.google.com/api-keys

## 実行ログ機能 (SQLite)

OCR・翻訳の履歴を SQLite に記録し、アプリ内のログビューアで時系列閲覧できる。詳細仕様は [FocusTranslator_LOG_SPECv0.1.md](FocusTranslator_LOG_SPECv0.1.md)。

- **既定OFF**。設定画面「実行ログを記録」をONにすると `%APPDATA%\FocusTranslator\logs\focustranslator.db` に記録が始まる。ONの間は**原文・訳文が平文で保存される**点に注意(外部送信はしない・ローカルのみ)。
- **デバッグモード**(別途ON): OCR実行時のキャプチャ画像を `logs\images\{id}.png` に保存し、ログと紐付ける(UIA経路は画像なし)。
- 記録内容: 認識(モード/方式/エンジン/時間/認識テキスト/画像)と、それに紐づく翻訳(エンジン/翻訳方向/時間/トークン数/訳文/**送信・受信の生JSON**)。1つの認識に複数の翻訳(エンジン切替のたび)が 1:N でぶら下がる。
- **APIキーのマスク**: 認証ヘッダは記録せず、生JSON中に設定済みキー文字列があれば `***MASKED***` に置換してから保存する。
- **ログビューア**(タスクトレイ →「ログビューア」、または設定画面の「ログビューアを開く」): 上段=認識一覧、中段=選択した認識の翻訳候補一覧、下段=選択した翻訳の原文/訳文・送信JSON・受信JSON をボタンで切替表示。デバッグ画像があれば下段右に縮小表示し、「画像を開く」で外部ビューアでも開ける。「ログを全削除」で全レコード+画像を消去。
- 保持上限(既定5000件)を超えると古い認識ログ・翻訳ログ・画像を自動削除する。
- SQLiteは rusqlite の bundled 機能で静的リンクするため外部DLL不要。`logs\focustranslator.db` は WAL モードのため、アプリ動作中でも VSCode の SQLite 拡張等で読み取り閲覧できる。画像は `logs\images\*.png` の実ファイルなので VSCode/エクスプローラーでそのままプレビュー可能。

## 翻訳元言語 / Geminiプロンプト

- 設定画面で翻訳元言語(既定 en)・訳先言語を選べる。原文がCJKを含み訳先がjaのときは自動で ja→en に反転する(ローカルONNXが ja⇄en のみ対応のため)。
- Geminiの翻訳プロンプト・OCR統合プロンプトを設定画面で編集できる。プレースホルダ `{{source_lang}}` `{{target_lang}}` `{{text}}` が設定値・原文で置換される。「既定に戻す」ボタンあり。

## 実測値 (開発環境でのスモークテスト)

- 押下 → 原文表示: UIA経路 約3ms / OCR経路 約200〜250ms
- アイドル時メモリ: 約13〜15MB

## SPECからの逸脱・未実装事項

- **ローカルONNX翻訳** (M3): モデル導入・推論とも実装済み(貪欲法デコード、KVキャッシュ未使用)。英→日方向はモデル自体の品質限界により訳文が粗い場合がある(上記参照)。
- **PaddleOCR** (M0): モデル導入(ダウンロード+チェックサム検証)は実装済み。推論(ONNX Runtime)は次版。
- **行矩形逸脱の監視**: ホールド中はマウス移動による再認識・表示位置の移動を行わない仕様(ユーザー要望により無効化)。次の行を見るには一度キーを離して押し直す。
- **候補展開・解説ボタン** (SPEC §10): 初版UIでは未実装(コピー・閉じる・チップ切替は実装済み)。
- **自動更新・インストーラ** (SPEC §13): 未実装(単一EXE配布)。
- ゲーム / HDR / 保護コンテンツ / 排他フルスクリーン / 縦書きは SPEC §15 のとおり対象外。
