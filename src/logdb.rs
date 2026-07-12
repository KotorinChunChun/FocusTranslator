// 実行ログのSQLite記録 (FocusTranslator_LOG_SPECv0.1.md / SPECv0.4 §8 スキーマv3)
// %APPDATA%\FocusTranslator\logs\focustranslator.db に4工程ツリー
// captures(入力) 1-N recognitions(認識) 1-N translations(翻訳) / explanations(解説) を記録する。
// デバッグモード時はキャプチャ画像を logs\images\{capture_id}.png に保存しパスを記録する。
// ログ既定OFF。ONのときのみ本モジュールが呼ばれる。
use crate::util;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

const SCHEMA_VERSION: i64 = 5;

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

/// 旧スキーマ(SCHEMA_VERSION未満)のDBを検出したら、DBファイルとimages配下PNGを破棄する (SPECv0.4 §8.4)。
fn discard_old_db_if_needed() -> Result<(), String> {
    let path = db_path();
    if !path.exists() {
        return Ok(());
    }
    let version: i64 = {
        let c = Connection::open(&path).map_err(|e| e.to_string())?;
        c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap_or(0)
    };
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    util::app_log(&format!("logdb: old schema v{version} detected, recreating"));
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.clone().into_os_string();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
    if let Ok(entries) = std::fs::read_dir(images_dir()) {
        for e in entries.flatten() {
            if e.path().extension().and_then(|x| x.to_str()) == Some("png") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(())
}

fn init_db() -> Result<Connection, String> {
    discard_old_db_if_needed()?;
    let conn = Connection::open(db_path()).map_err(|e| e.to_string())?;

    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         CREATE TABLE IF NOT EXISTS captures (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_ms INTEGER NOT NULL,
            mode TEXT NOT NULL,
            app_exe TEXT,
            app_title TEXT,
            uia_path TEXT,
            control_type TEXT,
            image_path TEXT,
            image_w INTEGER,
            image_h INTEGER
         );
         CREATE TABLE IF NOT EXISTS recognitions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            capture_id INTEGER NOT NULL REFERENCES captures(id) ON DELETE CASCADE,
            ts_ms INTEGER NOT NULL,
            method TEXT NOT NULL,
            engine TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            source_text TEXT,
            success INTEGER NOT NULL,
            error TEXT,
            tags TEXT,
            image_hash TEXT
         );
         CREATE TABLE IF NOT EXISTS translations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            recognition_id INTEGER NOT NULL REFERENCES recognitions(id) ON DELETE CASCADE,
            ts_ms INTEGER NOT NULL,
            engine TEXT NOT NULL,
            llm_profile TEXT,
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
         CREATE TABLE IF NOT EXISTS explanations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            recognition_id INTEGER NOT NULL REFERENCES recognitions(id) ON DELETE CASCADE,
            ts_ms INTEGER NOT NULL,
            llm_profile TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            input_text TEXT NOT NULL,
            explanation_text TEXT,
            success INTEGER NOT NULL,
            error TEXT,
            tokens_in INTEGER,
            tokens_out INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_recog_capture ON recognitions(capture_id);
         CREATE INDEX IF NOT EXISTS idx_recog_hash ON recognitions(image_hash, engine);
         CREATE INDEX IF NOT EXISTS idx_tr_recog ON translations(recognition_id);
         CREATE INDEX IF NOT EXISTS idx_ex_recog ON explanations(recognition_id);
         CREATE INDEX IF NOT EXISTS idx_ex_input ON explanations(input_text);
         CREATE INDEX IF NOT EXISTS idx_tr_req ON translations(request_json);",
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

/// 画像PNGを書き出して captures の image_path/image_w/image_h を更新する。
fn store_capture_image(guard: &Connection, capture_id: i64, img: &crate::capture::Captured) {
    let png = crate::capture::to_png(img);
    let rel = format!("images/{capture_id}.png");
    let path = images_dir().join(format!("{capture_id}.png")); // ディレクトリ作成込み
    if std::fs::write(&path, &png).is_ok() {
        let _ = guard.execute(
            "UPDATE captures SET image_path=?1, image_w=?2, image_h=?3 WHERE id=?4",
            rusqlite::params![rel, img.width as i64, img.height as i64, capture_id],
        );
    }
}

/// 入力(キャプチャ)ログを記録し capture_id を返す。画像は image が Some かつ debug 時のみPNG保存。
#[allow(clippy::too_many_arguments)]
pub fn log_capture(
    mode: &str,
    app_exe: Option<&str>,
    app_title: Option<&str>,
    uia_path: Option<&str>,
    control_type: Option<&str>,
    image: Option<&crate::capture::Captured>,
    debug: bool,
) -> Option<i64> {
    let m = conn()?;
    let guard = m.lock().ok()?;
    if let Err(e) = guard.execute(
        "INSERT INTO captures (ts_ms, mode, app_exe, app_title, uia_path, control_type, image_path, image_w, image_h)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL)",
        rusqlite::params![now_ms(), mode, app_exe, app_title, uia_path, control_type],
    ) {
        util::app_log(&format!("log_capture failed: {e}"));
        return None;
    }
    let id = guard.last_insert_rowid();
    if debug && let Some(img) = image {
        store_capture_image(&guard, id, img);
    }
    Some(id)
}

/// キャプチャ画像を編集後の画像で上書き差し替える (SPECv0.4 §4-4 トリミング適用時)。
pub fn replace_capture_image(capture_id: i64, image: &crate::capture::Captured) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    store_capture_image(&guard, capture_id, image);
}

/// 認識ログを記録し recognition_id を返す。image_hash は同一画像+同一エンジンでの
/// 再OCR判定に使う (SPECv0.4追補)。ハッシュを算出できない/不要な場合は None。
pub fn log_recognition(
    capture_id: i64,
    method: &str,
    engine: &str,
    duration_ms: u128,
    source_text: Option<&str>,
    error: Option<&str>,
    image_hash: Option<&str>,
) -> Option<i64> {
    let m = conn()?;
    let guard = m.lock().ok()?;
    let success = error.is_none();
    if let Err(e) = guard.execute(
        "INSERT INTO recognitions
            (capture_id, ts_ms, method, engine, duration_ms, source_text, success, error, tags, image_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
        rusqlite::params![
            capture_id, now_ms(), method, engine, duration_ms as i64,
            source_text, success as i64, error, image_hash
        ],
    ) {
        util::app_log(&format!("log_recognition failed: {e}"));
        return None;
    }
    Some(guard.last_insert_rowid())
}

/// 同一画像(image_hash)+同一エンジンでの成功済み認識結果があれば、その
/// (recognition_id, source_text) を返す (SPECv0.4追補: 再OCR/再ログを避けるためのキャッシュ)。
/// 最新のものを優先する。
pub fn find_cached_recognition(image_hash: &str, engine: &str) -> Option<(i64, String)> {
    let m = conn()?;
    let guard = m.lock().ok()?;
    guard
        .query_row(
            "SELECT id, source_text FROM recognitions
             WHERE image_hash = ?1 AND engine = ?2 AND success = 1 AND source_text IS NOT NULL
             ORDER BY ts_ms DESC LIMIT 1",
            rusqlite::params![image_hash, engine],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
}

/// 同一 request_json (APIキーはマスク済み) の成功済み翻訳結果があれば、その
/// (recognition_id, translated_text) を返す (SPECv0.4.8追補: 翻訳APIキャッシュ)。
/// 対象は request_json を記録するエンジン(deepl/google/llm)のみ。最新のものを優先する。
pub fn find_cached_translation(request_json: &str) -> Option<(i64, String)> {
    let m = conn()?;
    let guard = m.lock().ok()?;
    guard
        .query_row(
            "SELECT recognition_id, translated_text FROM translations
             WHERE request_json = ?1 AND success = 1 AND translated_text IS NOT NULL
             ORDER BY ts_ms DESC LIMIT 1",
            rusqlite::params![request_json],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
}

/// 同一 input_text (送信プロンプト全文) の成功済み解説結果があれば、その
/// (recognition_id, llm_profile, explanation_text) を返す (SPECv0.4.8追補: 解説APIキャッシュ)。
/// 最新のものを優先する。
pub fn find_cached_explanation(input_text: &str) -> Option<(i64, String, String)> {
    let m = conn()?;
    let guard = m.lock().ok()?;
    guard
        .query_row(
            "SELECT recognition_id, llm_profile, explanation_text FROM explanations
             WHERE input_text = ?1 AND success = 1 AND explanation_text IS NOT NULL
             ORDER BY ts_ms DESC LIMIT 1",
            rusqlite::params![input_text],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
}

/// 翻訳ログを記録する。
#[allow(clippy::too_many_arguments)]
pub fn log_translation(
    recognition_id: i64,
    engine: &str,
    llm_profile: Option<&str>,
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
        "INSERT INTO translations
            (recognition_id, ts_ms, engine, llm_profile, source_lang, target_lang, duration_ms,
             cache_hit, translated_text, success, error, request_json, response_json, tokens_in, tokens_out)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        rusqlite::params![
            recognition_id, now_ms(), engine, llm_profile, source_lang, target_lang,
            duration_ms as i64, cache_hit as i64, translated_text, success as i64,
            error, request_json, response_json, tokens_in, tokens_out
        ],
    ) {
        util::app_log(&format!("log_translation failed: {e}"));
    }
}

/// 解説ログを追記する (成功/失敗とも記録、置換はしない; SPECv0.4 §8.2.4)。
#[allow(clippy::too_many_arguments)]
pub fn log_explanation(
    recognition_id: i64,
    llm_profile: &str,
    duration_ms: u128,
    input_text: &str,
    explanation_text: Option<&str>,
    error: Option<&str>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let success = error.is_none();
    if let Err(e) = guard.execute(
        "INSERT INTO explanations
            (recognition_id, ts_ms, llm_profile, duration_ms, input_text, explanation_text,
             success, error, tokens_in, tokens_out)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            recognition_id, now_ms(), llm_profile, duration_ms as i64, input_text,
            explanation_text, success as i64, error, tokens_in, tokens_out
        ],
    ) {
        util::app_log(&format!("log_explanation failed: {e}"));
    }
}

/// 保持上限を超えた古い capture を削除する (配下の認識・翻訳・解説はCASCADE、PNGはファイル削除)。
pub fn rotate(max_records: u32) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let sql = "SELECT id, image_path FROM captures ORDER BY id DESC LIMIT -1 OFFSET ?1";
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
    for (id, image_path) in collected {
        if let Some(rel) = image_path {
            let _ = std::fs::remove_file(logs_dir().join(rel));
        }
        let _ = guard.execute("DELETE FROM captures WHERE id=?1", rusqlite::params![id]);
    }
}

// ---- ビューア用の読み出し ----

#[derive(Clone)]
pub struct CaptureRow {
    pub id: i64,
    pub ts_ms: i64,
    pub mode: String,
    pub app_exe: Option<String>,
    pub app_title: Option<String>,
    pub uia_path: Option<String>,
    pub control_type: Option<String>,
    pub image_path: Option<String>,
    pub image_w: Option<i64>,
    pub image_h: Option<i64>,
}

#[derive(Clone)]
pub struct RecogRow {
    pub id: i64,
    pub capture_id: i64,
    pub ts_ms: i64,
    pub method: String,
    pub engine: String,
    pub duration_ms: i64,
    pub source_text: String,
    pub success: bool,
    pub error: String,
    pub tags: String,
}

#[derive(Clone)]
pub struct TransRow {
    pub id: i64,
    pub recognition_id: i64,
    pub ts_ms: i64,
    pub engine: String,
    pub llm_profile: Option<String>,
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

#[derive(Clone)]
pub struct ExplainRow {
    pub id: i64,
    pub recognition_id: i64,
    pub ts_ms: i64,
    pub llm_profile: String,
    pub duration_ms: i64,
    pub input_text: String,
    pub explanation_text: String,
    pub success: bool,
    pub error: String,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
}

fn map_capture_row(r: &rusqlite::Row) -> rusqlite::Result<CaptureRow> {
    Ok(CaptureRow {
        id: r.get(0)?,
        ts_ms: r.get(1)?,
        mode: r.get(2)?,
        app_exe: r.get(3)?,
        app_title: r.get(4)?,
        uia_path: r.get(5)?,
        control_type: r.get(6)?,
        image_path: r.get(7)?,
        image_w: r.get(8)?,
        image_h: r.get(9)?,
    })
}

const CAPTURE_COLS: &str = "id, ts_ms, mode, app_exe, app_title, uia_path, control_type, image_path, image_w, image_h";

/// 入力履歴を新しい順に検索取得。query は配下の原文/訳文の部分一致、app_exe は完全一致。
/// 両方空文字なら全件 (最大 limit 件)。
pub fn search_captures(query: &str, app_exe: &str, limit: usize) -> Vec<CaptureRow> {
    let Some(m) = conn() else { return Vec::new() };
    let Ok(guard) = m.lock() else { return Vec::new() };
    let mut sql = format!("SELECT {CAPTURE_COLS} FROM captures WHERE 1=1");
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut p_idx = 1;

    if !query.is_empty() {
        sql.push_str(&format!(
            " AND (EXISTS (SELECT 1 FROM recognitions r WHERE r.capture_id = captures.id AND r.source_text LIKE ?{p_idx})
               OR EXISTS (SELECT 1 FROM recognitions r JOIN translations t ON t.recognition_id = r.id
                          WHERE r.capture_id = captures.id AND t.translated_text LIKE ?{p_idx}))"
        ));
        params.push(Box::new(format!("%{query}%")));
        p_idx += 1;
    }
    if !app_exe.is_empty() {
        sql.push_str(&format!(" AND app_exe = ?{p_idx}"));
        params.push(Box::new(app_exe.to_string()));
        p_idx += 1;
    }
    sql.push_str(&format!(" ORDER BY id DESC LIMIT ?{p_idx}"));
    params.push(Box::new(limit as i64));

    let p_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| &**b).collect();
    let Ok(mut stmt) = guard.prepare(&sql) else { return Vec::new() };
    match stmt.query_map(p_refs.as_slice(), map_capture_row) {
        Ok(iter) => iter.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

pub fn get_capture(id: i64) -> Option<CaptureRow> {
    let m = conn()?;
    let guard = m.lock().ok()?;
    guard
        .query_row(
            &format!("SELECT {CAPTURE_COLS} FROM captures WHERE id=?1"),
            rusqlite::params![id],
            |r| map_capture_row(r),
        )
        .ok()
}

pub fn get_unique_app_exes() -> Vec<String> {
    let Some(m) = conn() else { return Vec::new() };
    let Ok(guard) = m.lock() else { return Vec::new() };
    let mut stmt = match guard.prepare(
        "SELECT DISTINCT app_exe FROM captures WHERE app_exe IS NOT NULL AND app_exe != '' ORDER BY app_exe",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([], |r| r.get(0));
    if let Ok(iter) = rows {
        iter.flatten().collect()
    } else {
        Vec::new()
    }
}

/// 指定 capture に紐づく認識ログを時系列(古い順)取得
pub fn recognitions_for(capture_id: i64) -> Vec<RecogRow> {
    let Some(m) = conn() else { return Vec::new() };
    let Ok(guard) = m.lock() else { return Vec::new() };
    let mut stmt = match guard.prepare(
        "SELECT id, capture_id, ts_ms, method, engine, duration_ms, source_text, success, error, tags
         FROM recognitions WHERE capture_id=?1 ORDER BY id ASC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![capture_id], |r| {
        Ok(RecogRow {
            id: r.get(0)?,
            capture_id: r.get(1)?,
            ts_ms: r.get(2)?,
            method: r.get(3)?,
            engine: r.get(4)?,
            duration_ms: r.get(5)?,
            source_text: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            success: r.get::<_, i64>(7)? != 0,
            error: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            tags: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
        })
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// 指定認識に紐づく翻訳ログを時系列(古い順)取得
pub fn translations_for(recognition_id: i64) -> Vec<TransRow> {
    let Some(m) = conn() else { return Vec::new() };
    let Ok(guard) = m.lock() else { return Vec::new() };
    let mut stmt = match guard.prepare(
        "SELECT id, recognition_id, ts_ms, engine, llm_profile, source_lang, target_lang, duration_ms,
                cache_hit, translated_text, success, error, request_json, response_json, tokens_in, tokens_out
         FROM translations WHERE recognition_id=?1 ORDER BY id ASC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![recognition_id], |r| {
        Ok(TransRow {
            id: r.get(0)?,
            recognition_id: r.get(1)?,
            ts_ms: r.get(2)?,
            engine: r.get(3)?,
            llm_profile: r.get(4)?,
            source_lang: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            target_lang: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            duration_ms: r.get(7)?,
            cache_hit: r.get::<_, i64>(8)? != 0,
            translated_text: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
            success: r.get::<_, i64>(10)? != 0,
            error: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
            request_json: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
            response_json: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
            tokens_in: r.get(14)?,
            tokens_out: r.get(15)?,
        })
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// 指定認識に紐づく解説ログを時系列(古い順)取得
pub fn explanations_for(recognition_id: i64) -> Vec<ExplainRow> {
    let Some(m) = conn() else { return Vec::new() };
    let Ok(guard) = m.lock() else { return Vec::new() };
    let mut stmt = match guard.prepare(
        "SELECT id, recognition_id, ts_ms, llm_profile, duration_ms, input_text, explanation_text,
                success, error, tokens_in, tokens_out
         FROM explanations WHERE recognition_id=?1 ORDER BY id ASC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map(rusqlite::params![recognition_id], |r| {
        Ok(ExplainRow {
            id: r.get(0)?,
            recognition_id: r.get(1)?,
            ts_ms: r.get(2)?,
            llm_profile: r.get(3)?,
            duration_ms: r.get(4)?,
            input_text: r.get(5)?,
            explanation_text: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            success: r.get::<_, i64>(7)? != 0,
            error: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
            tokens_in: r.get(9)?,
            tokens_out: r.get(10)?,
        })
    });
    rows.map(|r| r.flatten().collect()).unwrap_or_default()
}

/// 「現在の解説」= 最新の成功した解説文を取得 (SPECv0.4 §8.2.4)。
pub fn latest_explanation(recognition_id: i64) -> Option<String> {
    let m = conn()?;
    let guard = m.lock().ok()?;
    guard
        .query_row(
            "SELECT explanation_text FROM explanations
             WHERE recognition_id=?1 AND success=1 AND explanation_text IS NOT NULL
             ORDER BY id DESC LIMIT 1",
            rusqlite::params![recognition_id],
            |r| r.get(0),
        )
        .ok()
}

/// 認識へのユーザー付与タグを取得
pub fn get_tags(recognition_id: i64) -> String {
    let Some(m) = conn() else { return String::new() };
    let Ok(guard) = m.lock() else { return String::new() };
    guard
        .query_row(
            "SELECT tags FROM recognitions WHERE id=?1",
            rusqlite::params![recognition_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// 認識へのユーザー付与タグを保存
pub fn set_tags(recognition_id: i64, tags: &str) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let _ = guard.execute(
        "UPDATE recognitions SET tags=?1 WHERE id=?2",
        rusqlite::params![tags, recognition_id],
    );
}

/// 全ログを削除 (テーブルDELETE + 画像全削除 + VACUUM)
pub fn clear_all() {
    let Some(m) = conn() else { return };
    if let Ok(guard) = m.lock() {
        let _ = guard.execute_batch("DELETE FROM captures; VACUUM;");
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

/// capture 1件を削除 (配下の認識・翻訳・解説はCASCADE、画像もファイル削除)。
pub fn delete_capture(id: i64) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let img: Option<String> = guard
        .query_row(
            "SELECT image_path FROM captures WHERE id=?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    if let Some(rel) = img {
        let _ = std::fs::remove_file(logs_dir().join(rel));
    }
    let _ = guard.execute("DELETE FROM captures WHERE id=?1", rusqlite::params![id]);
}

/// 認識1件を削除 (配下の翻訳・解説はCASCADE)。
pub fn delete_recognition(id: i64) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let _ = guard.execute("DELETE FROM recognitions WHERE id=?1", rusqlite::params![id]);
}

/// 翻訳1件を削除。
pub fn delete_translation(id: i64) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let _ = guard.execute("DELETE FROM translations WHERE id=?1", rusqlite::params![id]);
}

/// 解説1件を削除 (ログビューア拡張: 解説結果ブロックの選択削除)。
pub fn delete_explanation(id: i64) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let _ = guard.execute("DELETE FROM explanations WHERE id=?1", rusqlite::params![id]);
}

/// 認識結果テキストを上書き修正する (SPECv0.4: オーバーレイインライン編集用)
pub fn update_recog_text(recog_id: i64, new_text: &str) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let _ = guard.execute(
        "UPDATE recognitions SET source_text = ?1 WHERE id = ?2",
        rusqlite::params![new_text, recog_id],
    );
}

/// 翻訳結果テキストを上書き修正する (SPECv0.4: オーバーレイインライン編集用)。
/// 最新の(一番 ts_ms が新しい)翻訳結果を更新する。
pub fn update_trans_text(recog_id: i64, new_text: &str) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let _ = guard.execute(
        "UPDATE translations SET translated_text = ?1 WHERE recognition_id = ?2 AND ts_ms = (SELECT MAX(ts_ms) FROM translations WHERE recognition_id = ?2)",
        rusqlite::params![new_text, recog_id],
    );
}

/// 解説テキストを上書き修正する (SPECv0.4: オーバーレイインライン編集用)。
/// 最新の解説結果を更新する。
pub fn update_explain_text(recog_id: i64, new_text: &str) {
    let Some(m) = conn() else { return };
    let Ok(guard) = m.lock() else { return };
    let _ = guard.execute(
        "UPDATE explanations SET explanation_text = ?1 WHERE recognition_id = ?2 AND ts_ms = (SELECT MAX(ts_ms) FROM explanations WHERE recognition_id = ?2)",
        rusqlite::params![new_text, recog_id],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // FOCUSTRANSLATOR_DATA_DIR で隔離した環境でのみ動く。1プロセス1DBのため単一テストに集約。
    #[test]
    fn tree_record_cascade_rotate_clear() {
        // 環境変数切替中に onnx テスト等が config_dir() を参照しないよう直列化する
        let _guard = crate::util::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("ft_logdb_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::set_var("FOCUSTRANSLATOR_DATA_DIR", &tmp);
        }

        // 4工程ツリー: capture → recognition → translation / explanation
        let cid = log_capture(
            "hold", Some("game.exe"), Some("Game Window"), Some("Root > Panel"), Some("Edit"), None, false,
        )
        .expect("capture id");
        let rid = log_recognition(cid, "ocr", "win", 200, Some("hello"), None, Some("hash-a")).expect("recognition id");
        log_translation(
            rid, "llm", Some("prof1"), "en", "ja", 300, false, Some("こんにちは"), None,
            Some("{\"req\":1}"), Some("{\"res\":2}"), Some(10), Some(5),
        );
        log_explanation(rid, "prof1", 400, "prompt text", Some("解説文"), None, Some(20), Some(15));
        log_explanation(rid, "prof1", 400, "prompt text", None, Some("timeout"), None, None);

        let caps = search_captures("", "", 10);
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].app_exe.as_deref(), Some("game.exe"));
        assert_eq!(caps[0].control_type.as_deref(), Some("Edit"), "コントロール種類が記録される");
        let recs = recognitions_for(cid);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].source_text, "hello");
        let trs = translations_for(rid);
        assert_eq!(trs.len(), 1);
        assert_eq!(trs[0].translated_text, "こんにちは");
        assert_eq!(trs[0].llm_profile.as_deref(), Some("prof1"));
        assert_eq!(trs[0].tokens_in, Some(10));
        let exps = explanations_for(rid);
        assert_eq!(exps.len(), 2, "解説は追記式で失敗も記録される");
        assert!(!exps[1].success);
        assert_eq!(latest_explanation(rid).as_deref(), Some("解説文"));

        // 検索: 原文/訳文の部分一致とexe名フィルタ
        assert_eq!(search_captures("hell", "", 10).len(), 1);
        assert_eq!(search_captures("こんにち", "", 10).len(), 1);
        assert_eq!(search_captures("no-match", "", 10).len(), 0);
        assert_eq!(search_captures("", "game.exe", 10).len(), 1);
        assert_eq!(search_captures("", "other.exe", 10).len(), 0);

        // タグ (recognitions 側)
        set_tags(rid, "重要");
        assert_eq!(get_tags(rid), "重要");
        assert_eq!(recognitions_for(cid)[0].tags, "重要");

        // 再OCR相当: 同じ capture に認識行を追加
        let rid2 = log_recognition(cid, "ocr", "paddle", 100, Some("hello2"), None, Some("hash-b")).unwrap();
        assert_eq!(recognitions_for(cid).len(), 2);

        // 同一画像+同一エンジンの既存認識はキャッシュとして取得できる (再OCR回避, SPECv0.4追補)
        assert_eq!(find_cached_recognition("hash-a", "win"), Some((rid, "hello".to_string())));
        assert_eq!(find_cached_recognition("hash-a", "paddle"), None, "エンジンが違えばヒットしない");
        assert_eq!(find_cached_recognition("hash-x", "win"), None, "ハッシュが違えばヒットしない");

        // 翻訳・解説のDBキャッシュ検索 (SPECv0.4.8追補: API消費回避)
        assert_eq!(
            find_cached_translation("{\"req\":1}"),
            Some((rid, "こんにちは".to_string())),
            "同一request_jsonの成功済み翻訳がヒットする"
        );
        assert_eq!(find_cached_translation("{\"req\":no-match}"), None);
        assert_eq!(
            find_cached_explanation("prompt text"),
            Some((rid, "prof1".to_string(), "解説文".to_string())),
            "同一input_textの成功済み解説がヒットする(失敗ログは無視)"
        );
        assert_eq!(find_cached_explanation("no-match"), None);

        // CASCADE削除: recognition を消すと配下の翻訳・解説も消える
        delete_recognition(rid);
        assert_eq!(translations_for(rid).len(), 0);
        assert_eq!(explanations_for(rid).len(), 0);
        assert_eq!(recognitions_for(cid).len(), 1);

        // CASCADE削除: capture を消すと配下の認識も消える
        delete_capture(cid);
        assert_eq!(recognitions_for(cid).len(), 0);
        assert_eq!(translations_for(rid2).len(), 0);

        // ローテーション: captures 件数で絞り、配下はCASCADE
        let mut last_rid = 0;
        for i in 0..4 {
            let c = log_capture("hold", None, None, None, None, None, false).unwrap();
            last_rid = log_recognition(c, "ocr", "win", 100, Some(&format!("line{i}")), None, None).unwrap();
        }
        rotate(2);
        assert_eq!(search_captures("", "", 100).len(), 2, "rotate should cap captures");
        // 最新の capture の認識は残っている
        assert_eq!(recognitions_for(search_captures("", "", 1)[0].id).last().map(|r| r.id), Some(last_rid));

        // 全削除
        clear_all();
        assert_eq!(search_captures("", "", 100).len(), 0);

        // 他テスト(onnx等)が実データディレクトリを参照できるよう環境変数を戻す
        unsafe {
            std::env::remove_var("FOCUSTRANSLATOR_DATA_DIR");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
