#!/usr/bin/env bash
# FocusTranslator ビルド検証ループ。
# 実行中のexeをkillしてから build -> test -> clippy を順に走らせ、
# すべて通れば起動して常駐させたままにする(ユーザーの既定運用に合わせる)。
# 途中で失敗したら即座に中断し、その工程の出力を見せて終了する。
set -uo pipefail

echo "===== 実行中のexeを終了 ====="
taskkill //IM focus-translator.exe //F 2>&1 || true

echo "===== cargo build ====="
cargo build 2>&1 | tail -150
build_status=${PIPESTATUS[0]}
if [ "$build_status" -ne 0 ]; then
  echo "==> ビルド失敗。上記のエラーを確認してください。"
  exit 1
fi

echo "===== cargo test ====="
cargo test 2>&1 | tail -150
test_status=${PIPESTATUS[0]}
if [ "$test_status" -ne 0 ]; then
  echo "==> テスト失敗。上記の失敗テストを確認してください。"
  exit 1
fi

echo "===== cargo clippy --all-targets ====="
cargo clippy --all-targets 2>&1 | tail -150
clippy_status=${PIPESTATUS[0]}
if [ "$clippy_status" -ne 0 ]; then
  echo "==> clippyでエラー(または警告)が検出されました。上記を確認してください。"
  exit 1
fi

echo "===== 起動 ====="
( ./target/debug/focus-translator.exe & )
sleep 1
if tasklist | grep -qi focus-translator.exe; then
  echo "==> 起動確認OK。常駐させたままにします。"
else
  echo "==> 警告: 起動確認できませんでした(tasklistにプロセスが見つかりません)。"
fi
