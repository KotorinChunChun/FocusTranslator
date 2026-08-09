# FocusTranslator AI Context

この文書は、FocusTranslatorを初めて変更するAIエージェント向けの構成・設計メモです。利用者向けの操作説明は `README.md`、版ごとの要求と実装結果は `dev/CHANGE_LOG/vX.X.X-req.md` / `vX.X.X-imp.md` を参照してください。

## モジュールインデックス

### 入力・認識

- `capture.rs` / `capture_plan.rs` / `uia.rs`: カーソル下や選択範囲のキャプチャ計画、UI Automationによる文字列・画面情報の取得。
- `ocr.rs`: OneOCR、Windows OCR、PaddleOCR、LLM/VLMを共通のOCR結果へ正規化する上位経路。
- `oneocr.rs` / `paddle_ocr.rs`: 各OCRエンジン固有の実装。

### 翻訳・LLM

- `translate.rs`: ONNX、翻訳API、LLMプロファイルを共通の翻訳結果へ正規化する上位経路。
- `llm_api.rs`: `ApiProfile`を受け、HTTP API方式とCLI方式を振り分ける共通入口。キャッシュ用リクエスト表現と接続確認も同じ境界で振り分ける。
- `llm_cli.rs`: Codex、Claude、GitHub Copilot、Gemini、KimiのCLI検出、非対話コマンド構築、一時ファイル、タイムアウト、出力・usage正規化を担当する。HTTPや画面表示の責務は持たない。
- `onnx_translate.rs`: ローカルONNX翻訳モデルの推論。
- `llama_server.rs`: アプリ内で管理するllama.cppサーバーのライフサイクル。

### 設定・画面・処理制御

- `config.rs`: `ApiType` / `ApiProfile`を含む永続設定、既定値、設定移行。CLIプロファイルは初回移行時に一度だけ追加し、利用者が削除したものを再作成しない。
- `settings.rs`: Win32設定画面。LLMプロファイルではAPIのURL・キー入力と、CLIの自動検出・手動パス・導入案内を同じ編集領域で切り替える。
- `worker.rs`: キャプチャ後のOCR・翻訳・解説処理とログ記録を調停する。
- `overlay.rs` / `chip_handler.rs`: 結果表示と、OCR・翻訳・解説エンジン／プロファイル別チップからの再実行。
- `logdb.rs` / `logviewer.rs`: キャッシュを含むSQLite履歴とログ画面。

## 実装の詳細・既知の制約

### CLIを既存LLMプロファイルへ統合する理由

OCR・翻訳・解説はすでに `llm_api::call` と `LlmResponse` を共通利用している。CLI専用の上位経路を増やさず、この境界でHTTP／CLIを振り分けることで、プロンプト、外部送信同意、キャッシュ、ログ、トークン表示、プロファイル別チップの挙動を揃えている。CLI失敗時に別エンジンへ暗黙に切り替えてはならない。

### CLI子プロセスの安全境界

各呼び出しは専用の一時ディレクトリをcwdとして起動し、画像など必要な入力だけを配置する。CLIごとのread-only／ツール制限を指定し、120秒で終了しない子プロセスは停止する。ユーザー由来のプロンプトはWindowsの `.cmd` 層で再解釈されないよう、Codex・Claude・Gemini・Kimiでは標準入力、Copilotでは読み取り専用の一時 `request.txt` を使う。モデル名だけは引数に必要なので許可文字を限定する。新しいCLIや引数を追加するときも、ユーザー文字列をコマンドラインへ直接連結しないこと。

Codex CLI 0.147.0では `--ask-for-approval never` は `exec` サブコマンドより前のグローバル引数として渡す。アプリの作業ディレクトリはGitリポジトリではないため `--skip-git-repo-check` が必要であり、会話履歴を残さないため `--ephemeral` も指定する。この並びは実アカウントprobeと引数順序テストで固定している。

### 認証・利用上限の扱い

アプリが行うのはPATH検出、手動実行ファイル指定、バージョン確認、公式導入ページの案内までである。インストール、OAuthログイン、契約状態や残量の取得は各CLIへ委ねる。未導入、未ログイン、画像非対応、利用上限などはCLIの失敗を明示し、成功扱いや自動フォールバックにしない。usageは構造化出力から取得できた場合だけ既存ログへ保存する。

### CLIプロファイル移行

`Config.cli_profiles_seeded` は既存利用者へ5種類の既定CLIプロファイルを一度だけ加えるための移行フラグである。単純に「足りない名前を毎回追加」すると、利用者が意図的に削除したプロファイルを起動のたびに復活させるため、このフラグを維持すること。

## SPECからの逸脱・未実装事項

- 5種類すべてのコマンド構築・出力解析は単体テスト済み。Codex CLI 0.147.0はChatGPTログイン済み環境で翻訳・解説・画像OCRの実アカウントprobeにも成功している。Claude/Copilot/Gemini/Kimiは導入・ログイン済み環境でのE2E確認が残る。
- CLI配布元の引数や構造化出力形式が変更された場合は、各社の公式リファレンスを確認して `llm_cli.rs` と解析テストを更新する必要がある。
