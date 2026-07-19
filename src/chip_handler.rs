// チップ操作ハンドラ (SPEC v0.3 §8)
// オーバーレイのチップボタン押下時の処理をまとめたモジュール。
// app_state.rs から分離して可読性を向上させる。
use crate::app_state::{self, Mode, with_app, main_hwnd, sync_overlay};
use crate::config::Config;
use crate::engine;
use crate::overlay;
use crate::util;

use windows::Win32::Foundation::{POINT, RECT};
use windows::core::w;

/// handle_chip 冒頭でAppから取り出す状態一式。
/// App の借用を解放してからダイアログ表示やワーカー起動を行うためのスナップショット。
struct ChipCtx {
    cfg: Config,
    source: String,
    last_img: Option<std::sync::Arc<crate::capture::Captured>>,
    last_focus: crate::ocr::Focus,
    origin: POINT,
    target: isize,
    main: isize,
    anchor: (i32, i32),
    cur_tr: String,
    recog_id: Option<i64>,
    capture_id: Option<i64>,
    held_ctx: crate::worker::AppContext,
}

/// 現在のアプリ状態からプロンプト置換用コンテキストを組み立てる (SPECv0.4 §7.1)。
/// translated_text / tr_engine は最新の翻訳結果があるときのみ値を持つ。
fn prompt_ctx_from_app(original: &str) -> crate::config::PromptContext {
    with_app(|app| crate::config::PromptContext {
        original_text: original.to_string(),
        translated_text: app.translation.clone().unwrap_or_default(),
        app_title: app.app_title.clone(),
        app_exe: app.app_exe.clone(),
        uia_path: app.uia_path.clone(),
        ocr_engine: if app.via_uia { String::new() } else { app.cur_ocr.clone() },
        tr_engine: if app.translation.is_some() { app.cur_tr.clone() } else { String::new() },
    })
    .unwrap_or_default()
}

/// テキストを原文として採用し再翻訳する共通処理。UIAパスノードチップと「選択中の文字列」
/// チップのどちらも、OCRを介さずテキストをそのまま採用する点で同じ形になるため共用する。
fn adopt_text_and_retranslate(text: String, cur_tr: String, recog_id: Option<i64>, main: isize) {
    let new_gen = with_app(|app| {
        app.generation += 1;
        app.mode = Mode::Pinned;
        app.source = text.clone();
        app.via_uia = true;
        app.translation = None;
        let engine_name = engine::tr_display_name(&app.cur_tr, &app.cfg);
        app.status = Some(format!("{} で翻訳中…", engine_name));
        app.busy = true;
        sync_overlay(app);
        app.generation
    })
    .unwrap_or(0);
    let cfg2 = Config::load();
    let pc = prompt_ctx_from_app(&text);
    crate::worker::retranslate(new_gen, cur_tr, cfg2, text, main, recog_id, pc);
}

/// 画像編集モードの「編集終了」確定処理: 編集セッション中に確定した最終画像を
/// App/DBへ反映し、同一capture配下で再認識する (SPECv0.4 §4-3, §8.2.1)。
/// 「選択範囲を残す/消す」「元に戻す」は編集セッション内(overlay.rs)で完結し、
/// OCR/翻訳の再実行はここ(編集終了時)でのみ行う。
fn commit_edited_image(new_img: std::sync::Arc<crate::capture::Captured>) {
    let Some((cap_id, ocr_engine, cur_tr2, cfg2, main2, anchor2, held_ctx)) =
        with_app(|app| {
            app.last_img = Some(new_img.clone());
            app.generation += 1;
            app.mode = Mode::Pinned;
            app.status = Some("再認識中…".into());
            app.busy = true;
            sync_overlay(app);
            (
                app.capture_id,
                app.cur_ocr.clone(),
                app.cur_tr.clone(),
                app.cfg.clone(),
                app.main.0 as isize,
                app.anchor,
                app.held_ctx(),
            )
        })
    else {
        return;
    };
    if cfg2.log_enabled && cfg2.debug_mode
        && let Some(cid) = cap_id
    {
        crate::logdb::replace_capture_image(cid, &new_img);
    }
    let new_gen = with_app(|app| app.generation).unwrap_or(0);
    // クロップ済みの新しい画像が対象のため、行/段落の絞り込みは行わず全体をOCRする
    crate::worker::reocr(crate::worker::ReocrJob {
        generation: new_gen,
        capture_id: cap_id,
        img: Some(new_img),
        focus: crate::ocr::Focus::All,
        x: 0,
        y: 0,
        target: 0,
        ocr_engine,
        tr_engine: cur_tr2,
        cfg: cfg2,
        main: main2,
        anchor: anchor2,
        ctx: held_ctx,
        force_pin: true,
        perf_label: "reocr_edited",
    });
}

/// 画像編集のUndo履歴を1段階巻き戻す(編集セッション内で完結、OCRは再実行しない)。
/// 「元に戻す」チップと Ctrl+Z ポーリング(app_state::tick)の両方から呼ばれる共通処理。
pub fn perform_edit_undo() {
    if overlay::undo_edit() {
        with_app(sync_overlay);
    }
}

/// 編集アクション (選択範囲を残す/消す) を実行してオーバーレイを再描画する。
/// いずれも編集モードは終了せず作業中画像だけを差し替える (OCR/翻訳の再実行は編集終了時のみ)。
/// 失敗時はエラー内容をバッジ表示する。
fn run_edit_action(action: impl FnOnce() -> Result<(), String>) {
    let badge = action().err();
    with_app(|app| {
        if let Some(msg) = badge {
            app.badge = Some(msg);
        }
        sync_overlay(app);
    });
}

/// 画像編集モード中に禁止するチップか (OCR/翻訳エンジン切替・解説・言語反転・
/// UIAノード・テキスト編集)。編集セッションと再認識の衝突によるUI操作不能を防ぐ。
/// overlay_layout 側の無効表示と対で、ハンドラ側でも二重にガードする。
fn forbidden_while_editing(id: usize) -> bool {
    // id < CHIP_COPY はOCRチップ(CHIP_OCR_BASE..)と翻訳チップ(CHIP_TR_BASE..)の全域
    id < overlay::CHIP_COPY
        || matches!(
            id,
            overlay::CHIP_EXPLAIN
                | overlay::CHIP_EXPLAIN_QUICK
                | overlay::CHIP_SWAP_LANG
                | overlay::CHIP_EDIT_SRC
                | overlay::CHIP_EDIT_TR
                | overlay::CHIP_EDIT_EXP
                | overlay::CHIP_SELECTED_TEXT
        )
        || id >= overlay::CHIP_UIA_NODE_BASE
}

/// チップ押下 (SPEC §8): 押下時点でピン留めし、再OCR/再翻訳を実行
pub fn handle_chip(id: usize) {
    // 画像編集中はエンジン切替等を受け付けない (SPECv0.4.9追補)
    if overlay::is_editing_image() && forbidden_while_editing(id) {
        return;
    }
    // フェーズ1: 状態取得(借用を解放してから同意ダイアログを出す)
    let Some(c) = with_app(|app| ChipCtx {
        cfg: app.cfg.clone(),
        source: app.source.clone(),
        last_img: app.last_img.clone(),
        last_focus: app.last_focus,
        origin: app.origin,
        target: app.target,
        main: app.main.0 as isize,
        anchor: app.anchor,
        cur_tr: app.cur_tr.clone(),
        recog_id: app.recog_id,
        capture_id: app.capture_id,
        held_ctx: app.held_ctx(),
    }) else {
        return;
    };

    match id {
        overlay::CHIP_COPY => {
            // 解説コピー: 解説文をコピーする (ボタンは解説表示中のみ出る)
            let text = with_app(|app| app.explanation.clone()).flatten().unwrap_or_default();
            if !text.is_empty() {
                util::set_clipboard_text(main_hwnd(), &text);
                with_app(|app| {
                    app.badge = Some("解説をコピーしました".into());
                    sync_overlay(app);
                });
            }
        }
        overlay::CHIP_COPY_SRC => {
            let (src, _) = overlay::current_text();
            if !src.is_empty() {
                util::set_clipboard_text(main_hwnd(), &src);
                with_app(|app| {
                    app.badge = Some("原文をコピーしました".into());
                    sync_overlay(app);
                });
            }
        }
        overlay::CHIP_COPY_TR => {
            let (_, tr) = overlay::current_text();
            if let Some(t) = tr {
                util::set_clipboard_text(main_hwnd(), &t);
                with_app(|app| {
                    app.badge = Some("訳文をコピーしました".into());
                    sync_overlay(app);
                });
            }
        }
        overlay::CHIP_COPY_INFO => {
            let (title, path) =
                with_app(|app| (app.app_title.clone(), app.uia_path.clone())).unwrap_or_default();
            let mut text = title;
            if !path.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&path);
            }
            if !text.is_empty() {
                util::set_clipboard_text(main_hwnd(), &text);
                with_app(|app| {
                    app.badge = Some("対象情報をコピーしました".into());
                    sync_overlay(app);
                });
            }
        }
        overlay::CHIP_CLOSE => {
            with_app(app_state::close_overlay);
            return;
        }
        overlay::CHIP_PIN => {
            with_app(|app| {
                if app.mode != Mode::Pinned {
                    app.mode = Mode::Pinned;
                } else {
                    app.mode = Mode::ShowingHold;
                }
                sync_overlay(app);
            });
            return;
        }
        overlay::CHIP_FORCE_PIN => {
            with_app(|app| {
                if app.mode != Mode::Pinned {
                    app.mode = Mode::Pinned;
                    sync_overlay(app);
                }
            });
            return;
        }
        overlay::CHIP_IMAGE => {
            // 展開中(画像編集モード中)に再度押した場合は「編集終了」と同じ扱いにする。
            if overlay::is_editing_image() {
                if let Some(final_img) = overlay::finish_edit_session() {
                    commit_edited_image(final_img);
                } else {
                    with_app(sync_overlay);
                }
                return;
            }
            // キャプチャ画像のインライン編集モードを開始する (SPECv0.4 §1-§2)
            // 編集中はホールドキーを離してもオーバーレイが閉じないよう、ピン留め状態へ移行する。
            let img = with_app(|app| app.last_img.clone()).flatten();
            if let Some(i) = img {
                overlay::enter_edit_mode(i);
                with_app(|app| {
                    app.mode = Mode::Pinned;
                    sync_overlay(app);
                });
            }
            return;
        }
        overlay::CHIP_EDIT_RECT => {
            overlay::set_edit_tool(overlay::EditTool::Rect);
            with_app(sync_overlay);
            return;
        }
        overlay::CHIP_EDIT_LASSO => {
            overlay::set_edit_tool(overlay::EditTool::Lasso);
            with_app(sync_overlay);
            return;
        }
        overlay::CHIP_EDIT_RESET => {
            overlay::reset_edit_selection();
            with_app(sync_overlay);
            return;
        }
        overlay::CHIP_EDIT_CANCEL => {
            // 編集終了: セッション中に変更があれば最終画像を確定して再認識する。
            // 変更が無ければ単なるキャンセルとして扱う(OCR/翻訳は再実行しない)。
            if let Some(final_img) = overlay::finish_edit_session() {
                commit_edited_image(final_img);
            } else {
                with_app(sync_overlay);
            }
            return;
        }
        overlay::CHIP_EDIT_APPLY => {
            // 選択範囲を残す(旧「切り抜き」): 選択範囲でクロップして作業中画像を差し替える
            run_edit_action(overlay::apply_crop_keep_selection);
            return;
        }
        overlay::CHIP_EDIT_ERASE => {
            // 選択範囲を消す: 選択範囲の内側を隣接色で塗りつぶす(サイズは変わらない)
            run_edit_action(overlay::erase_selection_action);
            return;
        }
        overlay::CHIP_EDIT_UNDO => {
            perform_edit_undo();
            return;
        }
        
        // テキスト編集ポップアップ
        overlay::CHIP_EDIT_SRC => {
            chip_edit_source(c);
            return;
        }
        overlay::CHIP_EDIT_TR => {
            chip_edit_translation(c.recog_id);
            return;
        }
        overlay::CHIP_EDIT_EXP => {
            chip_edit_explanation(c.recog_id);
            return;
        }

        overlay::CHIP_SWAP_LANG => {
            chip_swap_lang(c);
            return;
        }
        overlay::CHIP_EXPLAIN_QUICK => {
            chip_explain_quick(c);
            return;
        }
        overlay::CHIP_EXPLAIN => {
            chip_explain(c);
            return;
        }
        overlay::CHIP_SETTINGS => {
            let inst = with_app(|app| app.instance).unwrap_or_default();
            crate::settings::open(inst, main_hwnd());
            return;
        }
        overlay::CHIP_OPEN_LOG => {
            let inst = with_app(|app| app.instance).unwrap_or_default();
            crate::logviewer::open(inst);
            return;
        }
        _ => {}
    }

    if id < overlay::CHIP_TR_BASE {
        // OCRエンジン切替: 保持画像で再認識→現行エンジンで再翻訳 (SPEC §8)
        switch_ocr_engine(id, c);
    } else if (overlay::CHIP_TR_BASE..overlay::CHIP_COPY).contains(&id) {
        // 翻訳エンジン切替: 現在の原文を選択エンジンで再翻訳 (SPEC §8)
        switch_tr_engine(id, c);
    } else if id == overlay::CHIP_SELECTED_TEXT {
        // 選択中の文字列を(再)採用: 検出済みの選択テキストを原文として採用し再翻訳
        let Some(text) = with_app(|app| app.selected_text.clone())
            .flatten()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        adopt_text_and_retranslate(text, c.cur_tr, c.recog_id, c.main);
    } else if id >= overlay::CHIP_UIA_NODE_BASE {
        // UIAパスノード選択: そのノードのテキストを原文として採用し再翻訳
        let idx = id - overlay::CHIP_UIA_NODE_BASE;
        let Some(text) = with_app(|app| app.uia_nodes.get(idx).map(|n| n.text.trim().to_string()))
            .flatten()
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        adopt_text_and_retranslate(text, c.cur_tr, c.recog_id, c.main);
    }
}

/// 原文の編集ダイアログ (CHIP_EDIT_SRC): 確定した原文をDB・Appへ反映し再翻訳する
fn chip_edit_source(c: ChipCtx) {
    let (hwnd, text) = with_app(|app| (app.overlay, app.source.clone())).unwrap();
    if let Some(new_text) = crate::input_dialog::show(hwnd, "原文を編集", &text) {
        if new_text.is_empty() || new_text == c.source {
            return;
        }
        if let Some(rid) = c.recog_id {
            crate::logdb::update_recog_text(rid, &new_text);
        }
        let new_gen = with_app(|app| {
            app.source = new_text.clone();
            app.translation = None;
            // 読み取り結果(原文)が変わったので解説も破棄する (再度開けば同一なら復元される)
            app.explanation = None;
            app.explaining = false;
            app.mode = Mode::Pinned;
            app.status = Some("修正された原文で再翻訳中…".into());
            app.busy = true;
            sync_overlay(app);
            app.generation += 1;
            app.generation
        }).unwrap_or(0);

        let cfg2 = Config::load();
        let pc = prompt_ctx_from_app(&new_text);
        crate::worker::retranslate(new_gen, c.cur_tr, cfg2, new_text, c.main, c.recog_id, pc);
    }
}

/// 翻訳結果の編集ダイアログ (CHIP_EDIT_TR): 確定した訳文をDB・Appへ反映する
fn chip_edit_translation(recog_id: Option<i64>) {
    let (hwnd, text) = with_app(|app| (app.overlay, app.translation.clone().unwrap_or_default())).unwrap();
    if let Some(new_text) = crate::input_dialog::show(hwnd, "翻訳結果を編集", &text) {
        let prev_tr = with_app(|app| app.translation.clone()).flatten().unwrap_or_default();
        if !new_text.is_empty() && new_text != prev_tr {
            if let Some(rid) = recog_id {
                crate::logdb::update_trans_text(rid, &new_text);
            }
            with_app(|app| {
                app.translation = Some(new_text);
                app.mode = Mode::Pinned;
                sync_overlay(app);
            });
        }
    }
}

/// 解説の編集ダイアログ (CHIP_EDIT_EXP): 確定した解説をDB・Appへ反映する
fn chip_edit_explanation(recog_id: Option<i64>) {
    let (hwnd, text) = with_app(|app| (app.overlay, app.explanation.clone().unwrap_or_default())).unwrap();
    if let Some(new_text) = crate::input_dialog::show(hwnd, "解説を編集", &text) {
        let prev_exp = with_app(|app| app.explanation.clone()).flatten().unwrap_or_default();
        if !new_text.is_empty() && new_text != prev_exp {
            if let Some(rid) = recog_id {
                crate::logdb::update_explain_text(rid, &new_text);
            }
            with_app(|app| {
                app.explanation = Some(new_text);
                app.mode = Mode::Pinned;
                sync_overlay(app);
            });
        }
    }
}

/// 翻訳方向の反転 (CHIP_SWAP_LANG): en→ja ⇄ ja→en を入れ替えて再翻訳する
fn chip_swap_lang(c: ChipCtx) {
    if c.source.is_empty() {
        return;
    }
    let new_gen = with_app(|app| {
        std::mem::swap(&mut app.cfg.source_lang, &mut app.cfg.target_lang);
        app.cfg.save();
        app.generation += 1;
        app.mode = Mode::Pinned;
        app.translation = None;
        let engine_name = engine::tr_display_name(&app.cur_tr, &app.cfg);
        app.status = Some(format!("{} で翻訳中…", engine_name));
        app.busy = true;
        sync_overlay(app);
        app.generation
    })
    .unwrap_or(0);
    let cfg2 = Config::load();
    let pc = prompt_ctx_from_app(&c.source);
    crate::worker::retranslate(new_gen, c.cur_tr, cfg2, c.source, c.main, c.recog_id, pc);
}

/// 解説(即時) (CHIP_EXPLAIN_QUICK): 編集ダイアログを出さず既定プロンプトをそのまま送信する
fn chip_explain_quick(c: ChipCtx) {
    with_app(|app| {
        app.mode = Mode::Pinned;
        sync_overlay(app);
    });
    let Some(r_id) = c.recog_id else { return };
    if c.source.is_empty() {
        return;
    }
    // キャッシュ済みの解説があれば即表示する
    if let Some(expl) = crate::logdb::latest_explanation(r_id) {
        with_app(|app| {
            app.mode = Mode::Pinned;
            app.status = None;
            app.error_only = false;
            app.explanation = Some(expl);
            sync_overlay(app);
        });
        return;
    }
    let pc = prompt_ctx_from_app(&c.source);
    let prompt = crate::worker::build_explain_prompt(&c.cfg, &pc).unwrap_or_default();
    if prompt.is_empty() {
        with_app(|app| {
            app.badge = Some("LLM APIが設定されていません".into());
            sync_overlay(app);
        });
        return;
    }
    let profile = c.cfg.active_api_profile.clone();
    let new_gen = with_app(|app| {
        app.generation += 1;
        app.mode = Mode::Pinned;
        app.status = Some(format!("LLM:{} で解説を取得中…", profile));
        app.explaining = true;
        app.busy = true;
        sync_overlay(app);
        app.generation
    })
    .unwrap_or(0);
    let cfg2 = Config::load();
    crate::worker::explain(new_gen, r_id, cfg2, prompt, profile, c.main);
}

/// 解説 (CHIP_EXPLAIN): プロンプト編集ウィンドウ (モードB) を表示してから送信 (SPECv0.4.7 §2)
fn chip_explain(c: ChipCtx) {
    with_app(|app| {
        app.mode = Mode::Pinned;
        sync_overlay(app);
    });
    if let Some(r_id) = c.recog_id
        && !c.source.is_empty()
    {
        let dialog_pos = with_app(|app| {
            let mut r = RECT::default();
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(app.overlay, &mut r);
            }
            (r.left + 24, r.top + 24)
        })
        .unwrap_or_default();

        // 全プロファイルの解説テンプレートを渡す (テンプレート/送信内容の分離編集)
        let profiles: Vec<crate::prompt_edit::ProfilePrompt> = c.cfg
            .api_profiles
            .iter()
            .map(|p| crate::prompt_edit::ProfilePrompt {
                name: p.name.clone(),
                template: p.explain_prompt.clone(),
            })
            .collect();
        if profiles.is_empty() {
            with_app(|app| {
                app.badge = Some("LLM APIが設定されていません".into());
                sync_overlay(app);
            });
            return;
        }
        let active_idx = c.cfg
            .api_profiles
            .iter()
            .position(|p| p.name == c.cfg.active_api_profile)
            .unwrap_or(0);

        let pc = prompt_ctx_from_app(&c.source);
        let main_hwnd_val = c.main;
        let inst = with_app(|app| app.instance).unwrap_or_default();
        crate::prompt_edit::open(
            inst,
            main_hwnd(),
            Some(dialog_pos),
            crate::prompt_edit::PromptKind::Explain,
            profiles,
            active_idx,
            Some(pc),
            Box::new(|name, tmpl| {
                crate::prompt_edit::save_prompt_to_config(
                    crate::prompt_edit::PromptKind::Explain,
                    name,
                    tmpl,
                )
            }),
            Some(Box::new(move |edited_prompt, profile| {
                let new_gen = with_app(|app| {
                    app.generation += 1;
                    app.mode = Mode::Pinned;
                    app.status = Some(format!("LLM:{} で解説を取得中…", profile));
                    app.explaining = true;
                    app.busy = true;
                    sync_overlay(app);
                    app.generation
                })
                .unwrap_or(0);
                let cfg2 = Config::load();
                crate::worker::explain(new_gen, r_id, cfg2, edited_prompt, profile, main_hwnd_val);
            })),
        );
    }
}

/// OCR/翻訳エンジンチップの押下位置から選択エンジンを解決し、利用可否と初回同意を確認する。
/// チップの並びは「固定エンジン (llm除く) + 準備済みLLMプロファイル」(overlay_layout と対)。
/// 返り値: (選択キー, 固定エンジンか, 実際に設定するエンジンキー)。範囲外・利用不可・同意拒否は None。
fn resolve_engine_chip(idx: usize, fixed_keys: &[&str], cfg: &Config) -> Option<(String, bool, String)> {
    let mut keys = Vec::new();
    for k in fixed_keys.iter() {
        if *k != "llm" {
            keys.push(k.to_string());
        }
    }
    for prof in cfg.ready_api_profiles() {
        keys.push(prof.name.clone());
    }
    let selected_key = keys.get(idx)?.clone();
    let is_fixed = fixed_keys.iter().any(|k| *k == selected_key && *k != "llm");
    let target = if is_fixed { selected_key.clone() } else { "llm".to_string() };

    let available = if is_fixed {
        cfg.engine_available(&selected_key)
    } else {
        cfg.api_profiles.iter().find(|p| p.name == selected_key).is_some_and(|p| p.is_ready())
    };
    if !available {
        with_app(|app| {
            app.status = Some(engine_unavailable_msg(&selected_key));
            sync_overlay(app);
        });
        return None;
    }
    if is_fixed && !ensure_consent(&selected_key, cfg) {
        return None;
    }
    Some((selected_key, is_fixed, target))
}

/// OCRエンジン切替チップ: 保持画像で再認識→現行エンジンで再翻訳 (SPEC §8)
fn switch_ocr_engine(id: usize, c: ChipCtx) {
    let idx = id - overlay::CHIP_OCR_BASE;
    let Some((selected_key, is_fixed, target_ocr)) = resolve_engine_chip(idx, &engine::OCR_KEYS, &c.cfg)
    else {
        return;
    };
    let new_gen = with_app(|app| {
        app.generation += 1;
        app.mode = Mode::Pinned;
        if is_fixed {
            app.cur_ocr = selected_key.clone();
        } else {
            app.cur_ocr = "llm".to_string();
            app.cfg.active_api_profile = selected_key.clone();
        }
        app.cfg.save();
        app.status = Some("再認識中…".into());
        app.busy = true;
        sync_overlay(app);
        app.generation
    })
    .unwrap_or(0);
    let cfg2 = Config::load();
    crate::worker::reocr(crate::worker::ReocrJob {
        generation: new_gen,
        capture_id: c.capture_id,
        img: c.last_img,
        focus: c.last_focus,
        x: c.origin.x,
        y: c.origin.y,
        target: c.target,
        ocr_engine: target_ocr,
        tr_engine: c.cur_tr,
        cfg: cfg2,
        main: c.main,
        anchor: c.anchor,
        ctx: c.held_ctx,
        force_pin: false,
        perf_label: "reocr",
    });
}

/// 翻訳エンジン切替チップ: 現在の原文を選択エンジンで再翻訳 (SPEC §8)
fn switch_tr_engine(id: usize, c: ChipCtx) {
    if c.source.is_empty() {
        return;
    }
    let idx = id - overlay::CHIP_TR_BASE;
    let Some((selected_key, is_fixed, target_tr)) = resolve_engine_chip(idx, &engine::TR_KEYS, &c.cfg)
    else {
        return;
    };
    let new_gen = with_app(|app| {
        app.generation += 1;
        app.mode = Mode::Pinned;
        if is_fixed {
            app.cur_tr = selected_key.clone();
        } else {
            app.cur_tr = "llm".to_string();
            app.cfg.active_api_profile = selected_key.clone();
        }
        app.cfg.save();
        let engine_name = engine::tr_display_name(&app.cur_tr, &app.cfg);
        app.status = Some(format!("{} で翻訳中…", engine_name));
        app.translation = None;
        app.busy = true;
        sync_overlay(app);
        app.generation
    })
    .unwrap_or(0);
    let cfg2 = Config::load();
    let pc = prompt_ctx_from_app(&c.source);
    crate::worker::retranslate(new_gen, target_tr, cfg2, c.source, c.main, c.recog_id, pc);
}

fn engine_unavailable_msg(key: &str) -> String {
    match key {
        "paddle" => "PaddleOCRのモデルが未導入です。設定画面からインストールしてください".into(),
        "local" => "ローカル翻訳モデルが未導入です。設定画面からインストールしてください".into(),
        _ => "APIキーが未設定です。設定画面で設定してください".into(),
    }
}

/// 初回同意ダイアログ (SPEC §9.2)。同意済みなら true。
fn ensure_consent(engine_key: &str, cfg: &Config) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_YESNO, MessageBoxW};
    let (need, kind): (bool, &str) = match engine_key {
        "deepl" | "google" => (!cfg.consent_text, "text"),
        "llm" => (!cfg.consent_image || !cfg.consent_text, "image"),
        _ => (false, ""),
    };
    if !need {
        return true;
    }
    let msg = match kind {
        "text" => w!(
            "このエンジンはOCR済みの原文テキストを外部サービスへ送信します。\n初回のみ確認しています。許可しますか?"
        ),
        _ => w!(
            "このエンジンはキャプチャ画像と言語設定を外部サービスへ送信する可能性があります。\n初回のみ確認しています。許可しますか?"
        ),
    };
    let r = unsafe { MessageBoxW(Some(main_hwnd()), msg, w!("外部送信の同意"), MB_YESNO) };
    if r == IDYES {
        let mut c = Config::load();
        match kind {
            "text" => c.consent_text = true,
            _ => {
                c.consent_image = true;
                c.consent_text = true;
            }
        }
        c.save();
        with_app(|app| app.cfg = c);
        true
    } else {
        false
    }
}
