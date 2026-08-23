---
name: version-bump
description: >
  Bump FocusTranslator's version number and update date across all the
  places it's hardcoded (Cargo.toml package version, installer.iss
  AppVersion, src/settings.rs APP_UPDATE_DATE shown in the settings screen).
  Use this whenever the user asks to prepare a release, bump the version,
  or says something like "バージョンをvX.X.Xに上げて" / "更新日を直して" /
  "リリース準備して". Do NOT use this for dependency version bumps in
  Cargo.toml (that's a normal `cargo update`, unrelated to this skill).
  Pairs with the `finish-feature` skill (doc sync) and the GitHub release
  notes template under `.github/RELEASE_TEMPLATE.md` — a full release
  typically uses all three.
---

# FocusTranslator バージョン更新

## なぜこのスキルが必要か

FocusTranslator はバージョン番号を **3箇所に手書きで重複して持っている**
(単一の"バージョン管理ファイル"に一元化されていない):

1. `Cargo.toml` の `version`(`env!("CARGO_PKG_VERSION")` としてビルドに埋め込まれ、
   設定画面のバージョン表記に使われる)
2. `installer.iss` の `AppVersion`(Inno Setupインストーラのバージョン表記)
3. `src/settings.rs` の `APP_UPDATE_DATE`(設定画面左下に表示する更新日。バージョン
   番号とは別に手動更新が必要)

1つだけ更新して他を忘れる事故が起きやすいため、3箇所まとめて機械的に置換するスクリプトを
用意した。

## 使い方

```bash
bash .Codex/skills/version-bump/scripts/bump_version.sh <新バージョン> <更新日>
```

例:

```bash
bash .Codex/skills/version-bump/scripts/bump_version.sh 0.5.6 2026/8/15
```

- 新バージョンは `Cargo.toml` の書式に合わせ `0.5.6` のように先頭の `v` を付けない。
- 更新日は設定画面の表示書式 `2026/8/15` のまま渡す(スクリプト内部で `/` を含む前提の
  sed区切り文字を使っているため、この書式を変えるとスクリプト側の修正が要る)。
- `Cargo.toml` は `sed` の `0,/pattern/` レンジ指定で**最初に一致した`version = "..."`のみ**
  書き換える。`[package]` の version が依存クレート群(`windows = { version = "0.62.2" }` 等)
  より必ず先に出現する前提に依っているため、`Cargo.toml` の構造(`[package]` が最初のテーブル)
  を大きく変えていないか一度は目視確認すること。

## 実行後にすること

1. `cargo build` を実行し `Cargo.lock` を更新・コンパイルが通ることを確認する
   (このリポジトリでは `verify-build` スキルで build/test/clippy/再起動までまとめて行える)。
2. `git diff --stat` で `Cargo.toml` / `installer.iss` / `src/settings.rs` の3ファイルだけが
   変わっていることを確認する。
3. 機能追加を伴うリリースなら `finish-feature` スキルでドキュメント3点(dev/CHANGE_LOG、
   AI_CONTEXT.md、README.md)も同期する。
4. GitHubへリリースを作る場合は `.github/RELEASE_TEMPLATE.md` を土台にリリースノートを書く。

## 注意

- このスキルは**表記の更新のみ**を行う。`git tag`・`git push`・GitHub Releaseの作成・
  インストーラのビルド(ISCC)は行わない — それらは別途ユーザーの指示・承認を得てから行うこと
  (タグ付けやリリース公開は「共有された状態への影響が大きい操作」に当たる)。
- `Cargo.lock` はこのスクリプトでは更新しない(`cargo build`が自動で行うため)。
