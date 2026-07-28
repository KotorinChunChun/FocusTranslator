---
name: verify-build
description: >
  Build, test, lint, and relaunch FocusTranslator (this Rust/Win32 desktop
  app) after making Rust source changes. Use this whenever you've edited any
  .rs file, Cargo.toml, or Cargo.lock in this repo and want to confirm the
  change compiles cleanly, passes `cargo test`, is clippy-clean, and actually
  runs — not just "cargo build succeeded". Also use it as the final
  confirmation step before telling the user a fix or feature is done. Kills
  the currently-running focus-translator.exe first (required on Windows —
  cargo build fails while the exe is running and locked) and leaves the app
  running afterward so the user can try it immediately; this matches the
  project's established dev workflow (see feedback_dev_build_process memory).
---

# FocusTranslator ビルド検証ループ

## なぜこのスキルが必要か

FocusTranslator は Windows 常駐アプリで、実行中は `target/debug/focus-translator.exe`
がロックされているため `cargo build` はまず既存プロセスを `taskkill` してからでないと
失敗する。さらにこのプロジェクトでは、コード変更のたびに

1. 実行中のexeをkill
2. `cargo build`
3. `cargo test`
4. `cargo clippy --all-targets`(このリポジトリは警告ゼロを維持する方針)
5. すべて通ったら再起動し、常駐させたまま終える

という同じ手順を毎回繰り返す。手作業でBashコマンドを1つずつ打つと4〜5往復かかるが、
このスキルなら1回のツール呼び出しで完結する。

## 使い方

```bash
bash .Codex/skills/verify-build/scripts/verify_build.sh
```

- 途中の工程(build/test/clippy)のいずれかで失敗したら、その時点で中断し該当の出力を
  表示して終了する(exit code 1)。**その出力を元に原因を修正し、直してから再度呼び出す。**
  失敗を握りつぶして次の工程へ進めることはない。
- 全工程が通れば、exeを起動してプロセスの存在を `tasklist` で確認するところまで行う。
- ユーザーの承認を待たずにkill/起動してよい(既存の開発フロー上の合意事項)。

## 使うタイミング

- `.rs` ファイル、`Cargo.toml`、`Cargo.lock` を編集した直後。
- ユーザーへ「実装できました」「修正しました」と報告する**前**の最終確認として。
- 複数ファイルにまたがる変更を終えた区切りごと(1つの機能の実装が終わったタイミング等)。

## 注意

- `cargo build`/`cargo test`/`cargo clippy` はいずれも時間がかかることがある
  (依存クレートの初回コンパイル等)。タイムアウトは十分に長く取ること。
- clippy警告が新たに出た場合は黙って無視せず修正すること(このリポジトリの既存コードは
  警告ゼロの状態を維持している)。
- ドキュメントのみの変更(`.md`)ではこのスキルは不要。`finish-feature` スキールを参照。
