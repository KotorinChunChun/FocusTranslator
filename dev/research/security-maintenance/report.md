# セキュリティ保守調査

2026-11-01、担当版v0.5.6。新設Gitを横断棚卸しへ追加。開始HEAD656b23b9eb60f084fe045f4b97ec3f3abfd319d1、追跡変更なし。親Gitとの独立管理を維持する。

| 依存 | 旧版→修正版 |
|---|---|
| rustls | 0.23.41→0.23.46 |
| rustls-webpki | 0.103.13→0.103.16 |
| webpki-roots / webpki-root-certs | 各1.0.8→1.0.10 |
| pulldown-cmark | 0.13.4→0.13.5 |
| crossbeam-epoch | 0.9.18→0.9.20 |

6組だけを同系列修正し、公式metadata/lock/取得crateのchecksumを照合。全体63成功/2ignore/build成功。修正版6組の再監査指摘なし。詳細なfeature/実到達性、初回取得失敗、未実施GUI、復旧は[版別結果](../../history/v0.5.6/v0.5.6-imp.md)。

ZIP2→9は互換設計待ち、glibはWindows非選択、paste/proc-macro-errorは保守終了通知。実攻撃成立数にまとめない。ローカル導入先と稼働設定/プロセスは変更せず、配布/上書き/再起動なし。。

## 2026-11-03 新着評価と互換パッチ

RUSTSEC-2026-0433はENABLE_WIKILINKS時の二次時間解析。現行src/markdown.rsはENABLE_TABLESだけなので非到達だが、将来の構成にも安全な0.13.6へpulldown-cmark0.13.5から予防更新した。版差分1組、公式checksum一致、既存全体63成功/2ignore/build成功。導入先未反映・未配布・停止/再起動なし。[版別検証・復旧](../../history/v0.5.6/v0.5.6-imp.md)。

RUSTSEC-2026-0429はCVE-2026-115909/GHSA-q7v9-x93c-xqv3の別名。zip2.4.2の公式crateと一致するsrc/read.rsで、extractのsymlink-target容量確保がWindows/Unix双方の対象と確認。当アプリはそのextractを使わず、llama_install.rsでenclosed_name検証後に個別std::io::copyを行うため、当該確保経路は未到達。一方、展開量上限は未実装で、ZIP全般の安全性を保証しない。zip2→9等の互換設計と他のZIP公告は別件として残す。

[ZIP公式原文](https://raw.githubusercontent.com/RustSec/advisory-db/main/crates/zip/RUSTSEC-2026-0429.md)。


## 2026-11-10 SQLite同梱版のセキュリティ更新候補

同梱SQLiteのCVE-2026-119375への対応を、通常の作業ツリーと利用者DBから分離して検討した。利用経路、対象版の隔離検証結果、候補差分、復旧範囲は[版別報告](../../history/v0.5.6/v0.5.6-imp.md)に記録した。

## 2026-11-15 ZIPのヘッダー解釈差

CVE-2026-115905 / GHSA-m665-v7pm-7grjを個別に評価した。依頼元の訂正により、OSVの更新日時の変化を影響範囲の変更とは扱わない。現行zip 2.4.2は公式公告の影響範囲内だが、アプリ内では同じZipArchiveで名前確認と個別展開を行い、異なる読み手のヘッダー解釈差をつなぐ経路は未検出。9.0.5へのメジャー更新は保留し、製品コード・依存は変更しない。入力元、依存ソースの照合、既存テストの不足、将来の互換性確認と復旧案は[版別報告](../../history/v0.5.6/v0.5.6-imp.md#2026-11-15-zipのヘッダー解釈差に関する評価)に記録した。前回の別ZIP公告やSQLite案件の再実装は行わない。
