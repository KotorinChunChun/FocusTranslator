# FocusTranslator ビルド & カスタマイズガイド

---

本ドキュメントでは、FocusTranslator のビルド手順に加えて、自前で開発・ビルドしたい方や、モデル・サーバー設定を自由にカスタマイズ（GPU版サーバーの利用、外部モデルの手動導入、クラウドAPIの活用など）したい上級ユーザー方向けの解説を記載しています。

📖 利用者向けメインガイドは: [README.md](../README.md)

---

## 📑 目次

- [🛠️ 開発環境の準備](#️-開発環境の準備)
- [🏗️ ビルドとインストーラ作成](#️-ビルドとインストーラ作成)
- [📂 データ保存ディレクトリ構造](#-データ保存ディレクトリ構造)
- [📦 各種モデル・サーバーの手動入手方法と配置パス](#-各種モデルサーバーの手動入手方法と配置パス)
  - [1. ローカルLLM / VLM (llama.cpp)](#1-ローカルllm--vlm-llamacpp)
  - [2. ローカル翻訳モデル (ONNX / FuguMT)](#2-ローカル翻訳モデル-onnx--fugumt)
  - [3. ローカル高精度OCR (PaddleOCR / RapidOCR)](#3-ローカル高精度ocr-paddleocr--rapidocr)
- [⚡ GPU版サーバーや自前LLMサーバーとの連携](#-gpu版サーバーや自前llmサーバーとの連携)
- [☁️ クラウドAPIサービスの利用と設定](#️-クラウドapiサービスの利用と設定)
  - [1. 対応クラウドサービスと特徴](#1-対応クラウドサービスと特徴)
  - [2. APIキーの入手手順](#2-apiキーの入手手順)
  - [3. 設定画面での登録とカスタマイズ](#3-設定画面での登録とカスタマイズ)
  - [4. セキュリティとプライバシー仕様](#4-セキュリティとプライバシー仕様)

---

## 🛠️ 開発環境の準備

開発およびビルドを行うには、以下のツールが必要です。

1. **Rust 開発環境**
   - [rustup](https://rustup.rs/) を使用して最新の Stable ツールチェーン（`x86_64-pc-windows-msvc`）をインストールしてください。
2. **Inno Setup 6**（インストーラ作成時のみ必要）
   - [Inno Setup 6](https://jrsoftware.org/isinfo.php) を既定のパス（`C:\Program Files (x86)\Inno Setup 6\`）にインストールしてください。

---

## 🏗️ ビルドとインストーラ作成

### 1. ローカルでの実行・デバッグ

```bat
# 依存関係のチェック
cargo check

# 開発ビルドと実行
cargo run
```

### 2. リリースビルド

最適化されたバイナリをビルドします。

```bat
cargo build --release
```

成果物は `target\release\focus-translator.exe` に出力されます。

### 3. インストーラのビルド

Inno Setup Compiler (`ISCC.exe`) を使用して配布用インストーラを作成します。

```bat
cargo build --release
"C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer.iss
```

成果物: `Output\focus-translator-setup.exe`

---

## 📂 データ保存ディレクトリ構造

FocusTranslator の設定ファイル、ログ、ダウンロードしたモデルやサーバー本体は、すべてユーザーの AppData ディレクトリ内に集約されます。

- **保存先パス**: `%APPDATA%\FocusTranslator\` (`C:\Users\<ユーザー名>\AppData\Roaming\FocusTranslator\`)

```text
%APPDATA%\FocusTranslator\
├── config.json               # 総合設定ファイル（ホットキー・プロファイル等）
├── log.db                    # ログ履歴データベース (SQLite)
├── llama/                    # llama.cpp 関連
│   └── bin/
│       └── llama-server.exe  # llama.cpp サーバー本体
└── models/                   # 各種AIモデル
    ├── llm/                  # ローカルLLM/VLMモデル
    │   ├── gemma-4-E2B-it-Q4_0.gguf
    │   └── mmproj-gemma-4-E2B-it-Q8_0.gguf
    ├── onnx_translate/       # ローカル翻訳モデル
    │   └── fugu_mt/
    │       ├── ja_en_encoder.onnx / ja_en_decoder.onnx / ja_en_tokenizer.json
    │       └── en_ja_encoder.onnx / en_ja_decoder.onnx / en_ja_tokenizer.json
    └── paddleocr/            # 高精度OCRモデル
        ├── det.onnx
        ├── rec.onnx
        └── dict.txt
```

---

## 📦 各種モデル・サーバーの手動入手方法と配置パス

設定画面からの自動ワンクリックインストールのほか、ブラウザ等で手動ダウンロードして配置することも可能です。

### 1. ローカルLLM / VLM (llama.cpp)

#### サーバー本体 (`llama-server.exe`)

- **入手先**: [ggml-org/llama.cpp Releases (GitHub)](https://github.com/ggml-org/llama.cpp/releases)
- **ファイル**: `llama-bXXXX-bin-win-cpu-x64.zip` （またはご利用のGPUに合わせた `cuda` / `vulkan` 版zip）
- **配置先**: `%APPDATA%\FocusTranslator\llama\bin\llama-server.exe`
  - zipを展開し、`llama-server.exe`（および依存DLL群）を上記ディレクトリに配置します。

#### メインLLMモデル (Google Gemma 4 E2B)

- **開発元**: Google / GGUF変換: `ggml-org`
- **入手先**: [ggml-org/gemma-4-E2B-it-GGUF (Hugging Face)](https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF)
- **ダウンロードファイル**: `gemma-4-E2B-it-Q4_0.gguf` (約 2.84 GB)
- **配置先**: `%APPDATA%\FocusTranslator\models\llm\gemma-4-E2B-it-Q4_0.gguf`

#### 画像認識用プロジェクター (VLM / mmproj)

- **入手先**: 上記と同じ Hugging Face リポジトリ
- **ダウンロードファイル**: `mmproj-gemma-4-E2B-it-Q8_0.gguf` (約 557 MB)
- **配置先**: `%APPDATA%\FocusTranslator\models\llm\mmproj-gemma-4-E2B-it-Q8_0.gguf`

---

### 2. ローカル翻訳モデル (ONNX / FuguMT)

日本語・英語の双方向ローカル翻訳を行うための軽量ONNXモデル群です。

- **入手先**:
  - 日→英: [Kadonox/fugumt-ja-en-onnx (Hugging Face)](https://huggingface.co/Kadonox/fugumt-ja-en-onnx)
  - 英→日: [Kadonox/fugumt-en-ja-onnx (Hugging Face)](https://huggingface.co/Kadonox/fugumt-en-ja-onnx)
- **配置先**: `%APPDATA%\FocusTranslator\models\onnx_translate\fugu_mt\`
- **必要ファイル一覧 (6ファイル)**:
  - `ja_en_encoder.onnx` (`onnx/encoder_model_quantized.onnx` からリネーム)
  - `ja_en_decoder.onnx` (`onnx/decoder_model_merged_quantized.onnx` からリネーム)
  - `ja_en_tokenizer.json` (`tokenizer.json` からリネーム)
  - `en_ja_encoder.onnx` (`onnx/encoder_model_quantized.onnx` からリネーム)
  - `en_ja_decoder.onnx` (`onnx/decoder_model_merged_quantized.onnx` からリネーム)
  - `en_ja_tokenizer.json` (`tokenizer.json` からリネーム)

---

### 3. ローカル高精度OCR (PaddleOCR / RapidOCR)

- **入手先**: [RapidAI/RapidOCR v3.9.1 (ModelScope)](https://www.modelscope.cn/models/RapidAI/RapidOCR/summary)
- **配置先**: `%APPDATA%\FocusTranslator\models\paddleocr\`
- **必要ファイル一覧 (3ファイル)**:
  - `det.onnx` (検出: `ch_PP-OCRv4_det_mobile.onnx`)
  - `rec.onnx` (認識: `japan_PP-OCRv4_rec_mobile.onnx`)
  - `dict.txt` (辞書: `japan_dict.txt`)

---

## ⚡ GPU版サーバーや自前LLMサーバーとの連携

本ツール内蔵の自動導入機能は汎用性と互換性を重視して **CPU版の llama-server.exe** を採用しています。  
NVIDIA GPU (CUDA) や AMD GPU (Vulkan) を活かしてさらに高速化したい場合や、LM Studio / Ollama / 自前のサーバーを使用する場合は以下の方法でカスタマイズできます。

### 方法A: 置き換え（内蔵サーバーのGPU化）

1. [llama.cpp Releases](https://github.com/ggml-org/llama.cpp/releases) から GPU版（例: `bin-win-cuda-cu12.4-x64.zip` 等）をダウンロードします。
2. `%APPDATA%\FocusTranslator\llama\bin\` 内の `llama-server.exe` および DLL 類をダウンロードしたGPU版に上書き置き換えします。
3. 設定画面で「内蔵LLMサーバーを起動」をオンにすると、GPU処理対応の `llama-server.exe` が自動的に起動します。

### 方法B: 外部サーバーとの連携 (LM Studio / Ollama / 独自サーバー)

1. ご自身のPCまたはローカルネットワーク上のサーバーで LLM / VLM サーバー（OpenAI互換APIエンドポイント）を起動します。
   - 例: `http://localhost:1234/v1` (LM Studio) や `http://localhost:11434/v1` (Ollama)
2. FocusTranslator の設定画面を開き、「5. LLMプロファイル設定」で新しいプロファイルを追加します。
3. エンドポイントURL、モデル名、APIキーを設定することで、外部の強力なサーバーで解説・翻訳処理を実行させることができます。

---

## ☁️ クラウドAPIサービスの利用と設定

FocusTranslator では、ローカル処理だけでなく各種クラウドAPIを活用して、より高精度な翻訳や高度な画面解説（LLM/VLM）を利用できます。

### 1. 対応クラウドサービスと特徴

| サービス / API           | 主な用途              | 特長・おすすめ理由                                                                                        |
| :----------------------- | :-------------------- | :-------------------------------------------------------------------------------------------------------- |
| **Google Gemini API**    | OCR + 翻訳 + 画像解説 | **おすすめ**。Google AI Studio から開発者向けの無料枠キーを取得でき、高速かつ高品質な画面解説が可能です。 |
| **OpenAI API**           | OCR + 翻訳 + 画像解説 | GPT-4o / GPT-4o-mini 等を活用し、極めて精度の高い画面コンテキスト解説が得られます。                       |
| **Anthropic Claude API** | OCR + 翻訳 + 画像解説 | Claude 3.5 Sonnet 等を使用し、複雑なプログラミング・技術用語の解説に威力を発揮します。                    |
| **DeepL API**            | 翻訳専用              | 自然で人間味のある高品質な翻訳結果が得られます（Free / Pro キー対応）。                                   |
| **Google 翻訳 API**      | 翻訳専用              | 多数の言語に対応した安定した翻訳サービスです。                                                            |

---

### 2. APIキーの入手手順

#### 🔹 DeepL API (DeepL 翻訳 API)

最初の100万文字までは無料で使える **DeepL Developer API** プランがあります。

1. [DeepL Developer API](https://www.deepl.com/pro-api) に登録します。
2. アカウントページから認証キー（`:fx` で終わるキー）を取得します。

#### 🔹 Google Cloud Translation API (Google 翻訳 API)

毎月 50 万文字までは無料で使える **Google Cloud Translation API** があります。

1. [Google Cloud Console](https://console.cloud.google.com/) にログインし、プロジェクトを作成（または既存プロジェクトを選択）します。
2. 「APIとサービス」 > 「ライブラリ」を開き、**Cloud Translation API** を検索して「有効にする」をクリックします。
3. 「認証情報」画面を開き、**「認証情報を作成」 > 「APIキー」** を選択して発行された API キーをコピーします。

#### 🔹 Google AI Studio Gemini API（汎用LLM/VLM）

GoogleのGemini Flash等のAIモデルが利用できます。Google AI Studioに登録すると、開発者用のAPIキーが取得できます。無料利用枠でも検証には十分なリクエスト回数が提供されています。

1. [Google AI Studio](https://aistudio.google.com/) にアクセスして Google アカウントでログインします。
2. **「Get API key」** ボタンをクリックし、新しい API キーを作成してコピーします。

#### 🔹 OpenAI API（汎用LLM/VLM）

OpenAIのGPTシリーズ（GPT-4o / GPT-4o-mini 等）が利用できます。無償利用枠はなく、支払い情報（クレジットカード等）を登録した有料アカウントが必要です。

1. [OpenAI Platform](https://platform.openai.com/) にログインし、`API Keys` ページを開きます。
2. 新しいシークレットキーを作成してコピーします。

> 💡 **補足**: 適切なプロファイル設定を行えば、Anthropic Claude やその他の OpenAI 互換 API サービスも同様に利用可能です。

---

### 3. 設定画面での登録とカスタマイズ

タスクトレイのトレイアイコンを右クリックし、「設定」画面を開きます。

#### A. 翻訳エンジンの設定（DeepL / Google翻訳 / Gemini等）

1. 設定画面の **「4. 翻訳設定」** を開きます。
2. 既定の翻訳エンジンを選択し、入手した API キーを入力します。

#### B. LLM解説プロファイルの設定（Gemini / OpenAI / Claude）

1. 設定画面の **「5. LLMプロファイル設定」** を開きます。
2. 新規プロファイルを作成（または既存プロファイルを編集）し、プロバイダタイプ（Gemini / OpenAI互換 / Anthropic等）、APIキー、モデル名（例: `gemini-3.6-flash` や `gpt-4o-mini`）を設定します。
3. **プロンプトテンプレートの編集**:
   - `{{original_text}}` などの変数を組み込み、専門分野に特化した解説を行わせるプロンプトをカスタマイズできます。
   - トークン数の上限（Max Tokens）や画面全体のスクリーンショット送出の有無も個別に設定可能です。

---

### 4. セキュリティとプライバシー仕様

本ツールは画面やテキストを扱うため、クラウドAPI利用時のセキュリティ対策を徹底しています。

- **DPAPIによる暗号化保存**:
  - 設定画面で入力された API キー等の機密情報は、Windowsの **DPAPI (Data Protection API)** を使用してローカル環境で暗号化されて保存されます。設定ファイル (`config.json`) を直接開いても平文でキーが漏洩することはありません。
- **初回送信時の明示的同意ダイアログ**:
  - クラウドAPIへ初めてリクエストを送信する際、データが外部へ送信される旨のユーザー同意確認ダイアログが表示されます。同意されない限り、外部通信は一切行われません。
- **ログのマスキング**:
  - デバッグログや表示ログにおいて、APIキー等の秘匿情報が記録されないよう自動マスク処理が適用されます。
