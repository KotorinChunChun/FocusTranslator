---
name: finish-feature
description: >
  Sync FocusTranslator's three documentation files after implementing a
  feature or fix: the versioned request/implementation log under
  dev/end/vX.X.X, the AI-facing FocusTranslator_AI_CONTEXT.md, and the
  user-facing README.md. Use this at the end of a work session once the code
  changes are built/tested/committed (or about to be), especially when the
  user says something like "反映して" / "追記して" / "ドキュメントも直して"
  about recent changes, or when you've added new source files, new settings
  UI, new user-visible behavior, or changed module responsibilities. Do NOT
  use this mid-implementation — it's a wrap-up step, not a planning step.
---

# ドキュメント3点の同期

FocusTranslator は、機能追加のたびに性質の異なる3つのドキュメントを更新する慣習がある。
それぞれ読者と粒度が違うため、同じ内容をコピーするのではなく**書き分ける**こと。

## 1. `dev/end/vX.X.X 修正内容.md` — 依頼内容と実装内容の記録

- ファイル名の `X.X.X` は現在作業中のバージョン(`Cargo.toml` の `version` と一致させる)。
  存在しなければ新規作成せず、既存ファイルの末尾に追記する(このファイルの冒頭には
  ユーザーの元依頼が命令口調でそのまま残っている — それは書き換えない)。
- 末尾に `## 実装内容` という見出しを立て(無ければ)、そのセッションで実装した内容を
  **箇条書き・過去形の体言止め**で追記する。命令口調(「〜してください」)にしないこと。
  例: 「解説ブロックへのLLMプロファイル別チップの追加」であって
  「解説ブロックにLLMプロファイル別チップを追加してください」ではない。
- 1機能=1〜2行程度の粒度。実装の詳細(具体的な関数名やアルゴリズム)は
  `FocusTranslator_AI_CONTEXT.md` 側に譲り、ここでは「何を実装したか」の要約に留める。
- このファイルは `.gitignore` で `dev/` ごと除外されているため、コミット対象にはならない
  (それでよい — 作業ログとしてローカルに残すためのファイル)。

## 2. `FocusTranslator_AI_CONTEXT.md` — 次にこのリポジトリを触るAI向け

読者は「このコードベースを初めて触るAIエージェント」。実装の**設計判断の理由**を書く。

- 新規 `.rs` ファイルを作った場合は「モジュールインデックス」の該当セクション
  (1〜8のいずれか、無ければ新設)に1行追加する。
- 既存モジュールの責務が変わった場合(新しいフィールド、新しい呼び出し経路等)は、
  そのモジュールの既存の説明文を書き換える(別行に追記するのではなく、説明そのものを
  最新の状態に更新する)。
- アーキテクチャ上の非自明な判断(例: 「なぜプロセスを1つにまとめたか」
  「なぜこのURLを固定せずAPIで解決するか」)は「実装の詳細・既知の制約」配下に
  小見出しを立てて残す。次にAIが同じ設計判断をしそうな箇所を手当てするのが目的であり、
  何を実装したかの列挙ではなく **なぜそう作ったか** を書くこと。
- 「SPECからの逸脱・未実装事項」に該当する項目があれば実装済みマークを更新する。

## 3. `README.md` — エンドユーザー向け

読者は「このアプリを使う一般ユーザー」。実装の中身ではなく**使い方・見え方**を書く。

- 実装の関数名・アーキテクチャ用語(プロファイル自動登録、コールバック等)は書かない。
  ユーザーが画面で見る文言(ボタン名、グループ名、チップの挙動)に合わせる。
- 設定画面のグループ構成が変わった場合(グループ追加/削除/統合)は、「3. 設定画面」章の
  グループ数・グループ一覧をコードの実際の構成と一致させる(食い違うと実装と乖離する)。
  設定画面のグループ名・グループ数は `src/settings.rs` の `group(h, inst, "N. ...", ...)`
  呼び出しを実際に確認して転記すること — 記憶や推測で書かない。
- OCR/翻訳エンジンの対応表、モデル導入セクションなど、機能表に新しい行や注記が要るか確認する。
- 既存の文体・見出し構成(絵文字は使わない、である調ではなく敬体)に合わせる。

## 実行順序の目安

1. まず `Cargo.toml` の version と `dev/end/` 配下の既存ファイル名を確認し、対象バージョンの
   ファイルを特定する。
2. 3ファイルとも、セッション内の会話全体(ユーザーの依頼と実際に行った実装)を見返して、
   実装したことの一覧を先に自分の中で確定させてから、3つの粒度に書き分けて反映する。
3. コード変更を伴わない(ドキュメントのみの)作業なので `verify-build` スキルは不要。
4. 変更後、`git status`/`git diff --stat` で意図した3ファイルだけが変わっているか確認する。
   `dev/` は gitignore 対象なので `git status` には出ない — それは正常。
