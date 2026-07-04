// 実行ログのSQLite記録 (FocusTranslator_LOG_SPECv0.1.md)
// %APPDATA%\FocusTranslator\logs\focustranslator.db に認識ログ・翻訳ログを記録する。
// デバッグモード時はキャプチャ画像を logs\images\{recognition_id}.png に保存しパスを記録する。
// ログ既定OFF。ONのときのみ本モジュールが呼ばれる。
use crate::util;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const SCHEMA_VERSION: i64 = 1;

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

pub fn logs_dir() -> PathBuf {
    let p = util::config_dir().join("logs");
    let _ = std::fs::create_dir_all(&p);
    p
}

pub fn images_dir() -> PathBuf {
    let p = logs_dir().join("images");
    let _ = std::fs::create_dir_all(&p);
    p
}

fn db_path() -> PathBuf {
    logs_dir().join("focustranslator.db")
}

/// DB接続を取得(初回はスキーマ作成)。失敗時は None(ログ機能は諦めるがアプリは継続)。
fn conn() -> Option<&'static Mutex<Connection>> {
    // OnceLock は失敗を保持できないため、初期化に失敗したら以後も None を返す
    if let Some(m) = DB.get() {
        return Some(m);
    }
    match init_db() {
        Ok(c) => Some(DB.get_or_init(|| Mutex::new(c))),
        Err(e) => {
            util::app_log(&format!("logdb init failed: {e}"));
            None
        }
    }
}

fn init_db() -> Result<Connection, String> {
    let conn = Connection::open(db_path()).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS recognition_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_ms INTEGER NOT NULL,
            mode TEXT NOT NULL,
            method TEXT NOT NULL,
            engine TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            source_text TEXT,
            success INTEGER NOT NULL,
            error TEXT,
            image_path TEXT,
            image_w INTEGER,
            image_h INTEGER
         );
         CREATE TABLE IF NOT EXISTS translation_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_ms INTEGER NOT NULL,
            recognition_id INTEGER,
            engine TEXT NOT NULL,
            source_lang TEXT,
            target_lang TEXT,
            duration_ms INTEGER NOT NULL,
            cache_hit INTEGER NOT NULL,
            translated_text TEXT,
            success INTEGER NOT NULL,
            error TEXT,
            request_json TEXT,
            response_json TEXT,
            tokens_in INTEGER,
            tokens_out INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_tr_recog ON translation_logs(recognition_id);",
    )
    .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 認識ログを記録し recognition_id を返す。画像は image(BGRA) が Some かつ debug 時のみPNG保存。
#[allow(clippy::too_many_arguments)]
pub fn log_recognition(
    mode: &str,
    method: &str,
    engine: &str,
    duration_ms: u128,
    source_text: Option<&str>,
    error: Option<&str>,
    image: Option<&crate::capture::Captured>,
    debug: bool,
) -> Option<i64> {
    let m = conn()?;
    let guard = m.lock().ok()?;
    let success = error.is_none();
    if let Err(e) = guard.execute(
        "INSERT INTO recognition_logs
            (ts_ms, mode, method, engine, duration_ms, source_text, success, error, image_path, image_w, image_h)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL)",
        rusqlite::params![
            now_ms(), mode, method, engine, duration_ms as i64,
            source_text, success as i64, error
        ],
    ) {
        util::app_log(&format!("log_recognition failed: {e}"));
        return None;
    }
    let id = guard.last_insert_rowid();

    // デバッグモード時のみ画像を保存
    if debug
        && let Some(img) = image {
            let png = crate::capture::to_png(img);
            let rel = format!("images/{id}.png");
            let path = images_dir().join(format!("{id}.png")); // ディレクトリ作成込み
            if std::fs::write(&path, &png).is_ok() {
                let _ = guard.execute(
                    "UPDATE recognition_logs SET image_path=?1, image_w=?2, image_h=?3 WHERE id=?4",
                    rusqlite::params![rel, img.width as i64, img.height as i64, id],
                );
            }
        }
    Some(id)
}

/// 翻訳ログを記録する。
#[allow(clippy::too_many_arguments)]
pub fn log_translation(
    recognition_id: Option<i64>,
    engine: &str,
    source_lang: &str,
    target_lang: &str,
    duration_ms: u128,
    cache_hit: bool,
    translated_text: Option<&str>,
    error: Option<&str>,
    request_json: Option<&str>,
    response_json: Option<&str>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let success = error.is_none();
    if let Err(e) = guard.execute(
        "INSERT INTO translation_logs
            (ts_ms, recognition_id, engine, source_lang, target_lang, duration_ms, cache_hit,
             translated_text, success, error, request_json, response_json, tokens_in, tokens_out)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            now_ms(), recognition_id, engine, source_lang, target_lang,
            duration_ms as i64, cache_hit as i64, translated_text, success as i64,
            error, request_json, response_json, tokens_in, tokens_out
        ],
    ) {
        util::app_log(&format!("log_translation failed: {e}"));
    }
}

/// 保持上限を超えた古い認識ログ(と紐づく翻訳ログ・画像)を削除する。
pub fn rotate(max_records: u32) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    // 上限を超える古いレコードのidと画像パスを取得
    let sql = "SELECT id, image_path FROM recognition_logs
               ORDER BY id DESC LIMIT -1 OFFSET ?1";
    let mut stmt = match guard.prepare(sql) {
        Ok(s) => s,
        Err(_) => return,
    };
    let collected: Vec<(i64, Option<String>)> = match stmt
        .query_map(rusqlite::params![max_records as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
        }) {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => Vec::new(),
    };
    drop(stmt);
    let mut old_ids: Vec<i64> = Vec::new();
    for (id, image_path) in collected {
        old_ids.push(id);
        if let Some(rel) = image_path {
            let _ = std::fs::remove_file(logs_dir().join(rel));
        }
    }
    for id in old_ids {
        let _ = guard.execute("DELETE FROM translation_logs WHERE recognition_id=?1", rusqlite::params![id]);
        let _ = guard.execute("DELETE FROM recognition_logs WHERE id=?1", rusqlite::params![id]);
    }
}

// ---- ビューア用の読み出し ----

#[derive(Clone)]
#[allow(dead_code)] // method はビューアで現状未表示だが将来の列追加・エクスポート用に保持
pub struct RecogRow {
    pub id: i64,
    pub ts_ms: i64,
    pub mode: String,
    pub method: String,
    pub engine: String,
    pub duration_ms: i64,
    pub source_text: String,
    pub success: bool,
    pub error: String,
    pub image_path: Option<String>,
}

#[derive(Clone)]
#[allow(dead_code)] // id はビューアで現状未表示だが将来の個別削除・参照用に保持
pub struct TransRow {
    pub id: i64,
    pub ts_ms: i64,
    pub engine: String,
    pub source_lang: String,
    pub target_lang: String,
    pub duration_ms: i64,
    pub cache_hit: bool,
    pub translated_text: String,
    pub success: bool,
    pub error: String,
    pub request_json: String,
    pub response_json: String,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

/// 認識ログを新しい順に最大 limit 件取得
pub fn recent_recognitions(limit: usize) -> Vec<RecogRow> {
    let Some(m) = conn() else { return Vec::new() };
    let Ok(guard) = m.lock() else { return Vec::new() };
    let mut stmt = match guard.prepare(
        "SELECT id, ts_ms, mode, method, engine, duration_ms, source_text, success, error, image_path
         FROM recognition_logs ORDER BY id DESC LIMIT ?1",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![limit as i64], |r| {
        Ok(RecogRow {
            id: r.get(0)?,
            ts_ms: r.get(1)?,
            mode: r.get(2)?,
            method: r.get(3)?,
            engine: r.get(4)?,
            duration_ms: r.get(5)?,
            source_text: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            success: r.get::<_, i64>(7)? != 0,
            error: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            image_path: r.get(9)?,
        })
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// 指定認識に紐づく翻訳ログを時系列(古い順)取得
pub fn translations_for(recognition_id: i64) -> Vec<TransRow> {
    let Some(m) = conn() else { return Vec::new() };
    let Ok(guard) = m.lock() else { return Vec::new() };
    let mut stmt = match guard.prepare(
        "SELECT id, ts_ms, engine, source_lang, target_lang, duration_ms, cache_hit,
                translated_text, success, error, request_json, response_json, tokens_in, tokens_out
         FROM translation_logs WHERE recognition_id=?1 ORDER BY id ASC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![recognition_id], |r| {
        Ok(TransRow {
            id: r.get(0)?,
            ts_ms: r.get(1)?,
            engine: r.get(2)?,
            source_lang: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            target_lang: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            duration_ms: r.get(5)?,
            cache_hit: r.get::<_, i64>(6)? != 0,
            translated_text: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            success: r.get::<_, i64>(8)? != 0,
            error: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
            request_json: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
            response_json: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
            tokens_in: r.get(12)?,
            tokens_out: r.get(13)?,
        })
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// 全ログを削除 (テーブルDELETE + 画像全削除 + VACUUM)
pub fn clear_all() {
    let Some(m) = conn() else { return };
    if let Ok(guard) = m.lock() {
        let _ = guard.execute_batch(
            "DELETE FROM translation_logs; DELETE FROM recognition_logs; VACUUM;",
        );
    }
    // images ディレクトリのPNGを削除
    if let Ok(entries) = std::fs::read_dir(images_dir()) {
        for e in entries.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("png") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // FOCUSTRANSLATOR_DATA_DIR で隔離した環境でのみ動く。1プロセス1DBのため単一テストに集約。
    #[test]
    fn record_read_rotate_clear() {
        let tmp = std::env::temp_dir().join(format!("ft_logdb_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::set_var("FOCUSTRANSLATOR_DATA_DIR", &tmp);
        }

        // 認識ログ + 翻訳ログ
        let rid = log_recognition("hold", "ocr", "win", 200, Some("hello"), None, None, false)
            .expect("recognition id");
        log_translation(
            Some(rid), "gemini", "en", "ja", 300, false, Some("こんにちは"), None,
            Some("{\"req\":1}"), Some("{\"res\":2}"), Some(10), Some(5),
        );

        let recs = recent_recognitions(10);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].source_text, "hello");
        let trs = translations_for(rid);
        assert_eq!(trs.len(), 1);
        assert_eq!(trs[0].translated_text, "こんにちは");
        assert_eq!(trs[0].tokens_in, Some(10));

        // ローテーション: さらに数件足して上限2に絞る
        for i in 0..3 {
            log_recognition("hold", "ocr", "win", 100, Some(&format!("line{i}")), None, None, false);
        }
        rotate(2);
        assert!(recent_recognitions(100).len() <= 2, "rotate should cap records");

        // 全削除
        clear_all();
        assert_eq!(recent_recognitions(100).len(), 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
