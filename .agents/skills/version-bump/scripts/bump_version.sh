#!/usr/bin/env bash
# FocusTranslator のバージョン表記を一括更新する。
# 使い方: bash bump_version.sh <新バージョン例:0.5.6> <更新日例:2026/8/1>
set -euo pipefail

if [ $# -ne 2 ]; then
    echo "使い方: bash bump_version.sh <新バージョン 例:0.5.6> <更新日 例:2026/8/1>" >&2
    exit 1
fi

NEW_VERSION="$1"
NEW_DATE="$2"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "$ROOT"

echo "== Cargo.toml (パッケージ本体のversionのみ。依存クレートのversionは触らない) =="
# 0,/pattern/ は「最初に一致した箇所まで」に絞る GNU sed のレンジ指定。
# Cargo.toml は [package] の version が常に依存クレート群より先に出るため、
# 最初の一致 = パッケージ自身の version になる。
sed -i "0,/^version = \".*\"/{s/^version = \".*\"/version = \"${NEW_VERSION}\"/}" Cargo.toml
grep -n "^version = " Cargo.toml | head -1

echo "== installer.iss (AppVersion) =="
sed -i "s/^AppVersion=.*/AppVersion=${NEW_VERSION}/" installer.iss
grep -n "^AppVersion=" installer.iss

echo "== src/settings.rs (APP_UPDATE_DATE) =="
# 更新日 "2026/7/31" のようにスラッシュを含むため、sed区切り文字は / ではなく | を使う。
sed -i "s|^const APP_UPDATE_DATE: &str = \".*\";|const APP_UPDATE_DATE: \&str = \"${NEW_DATE}\";|" src/settings.rs
grep -n "const APP_UPDATE_DATE" src/settings.rs

echo
echo "== 更新後の確認: 旧バージョン番号の残存チェック =="
echo "(Cargo.lock は cargo build で自動更新されるため対象外。dev/CHANGE_LOG 配下の"
echo " 過去バージョンの履歴ファイル名は意図的に残るため無視してよい)"
OLD_VERSION_GREP=$(grep -rn "version = \"" Cargo.toml | head -1 || true)
echo "現在のCargo.toml version行: ${OLD_VERSION_GREP}"

echo
echo "完了。次にすること:"
echo "  1. cargo build でCargo.lockを更新し、コンパイルが通ることを確認する"
echo "     (このリポジトリでは verify-build スキルでbuild/test/clippy/再起動までまとめて行う)"
echo "  2. git diff で Cargo.toml / installer.iss / src/settings.rs の3ファイルだけが"
echo "     変わっていることを確認する"
