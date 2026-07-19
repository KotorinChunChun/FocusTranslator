// チップ操作ハンドラ (SPEC v0.3 §8)
// オーバーレイのチップボタン押下時の処理をまとめたモジュール。
// app_state.rs から分離して可読性を向上させる。
use crate::app_state::{self, Mode, with_app, main_hwnd, sync_overlay};
use crate::config::Config;
use crate::engine;
use crate::overlay;
use crate::util;

use windows::Win32::Foundation::RECT;
use windows::core::w;

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
    let Some((cap_id, ocr_engine, cur_tr2, cfg2, main2, anchor2, app_title, app_exe, uia_path, uia_nodes, control_type, selected_text)) =
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
                app.app_title.clone(),
                app.app_exe.clone(),
                app.uia_path.clone(),
                app.uia_nodes.clone(),
                app.control_type.clone(),
                app.selected_text.clone(),
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
    crate::worker::reocr_edited(
        new_gen, cap_id, new_img, ocr_engine, cur_tr2, cfg2, main2, anchor2, app_title, app_exe,
        uia_path, uia_nodes, control_type, selected_text,
    );
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
    id < overlay::CHIP_TR_BASE
        || (id >= overlay::CHIP_TR_BASE && id < overlay::CHIP_COPY)
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
    let Some(info) = with_app(|app| {
        (
            app.cfg.clone(),
            app.source.clone(),
            app.last_img.clone(),
            app.last_focus,
            app.origin,
            app.target,
            app.main.0 as isize,
            app.anchor,
            app.cur_tr.clone(),
            app.recog_id,
            app.capture_id,
            app.app_title.clone(),
            app.app_exe.clone(),
            app.uia_path.clone(),
            app.uia_nodes.clone(),
            app.control_type.clone(),
            app.selected_text.clone(),
        )
    }) else {
        return;
    };
    let (
        cfg, source, last_img, last_focus, origin, target, main, anchor, cur_tr, recog_id, capture_id,
        held_app_title, held_app_exe, held_uia_path, held_uia_nodes, held_control_type, held_selected_text,
    ) = info;

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
            let (hwnd, text) = with_app(|app| (app.overlay, app.source.clone())).unwrap();
            if let Some(new_text) = crate::input_dialog::show(hwnd, "原文を編集", &text) {
                if new_text.is_empty() || new_text == source {
                    return;
                }
                if let Some(rid) = recog_id {
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
                crate::worker::retranslate(new_gen, cur_tr, cfg2, new_text, main, recog_id, pc);
            }
            return;
        }
        overlay::CHIP_EDIT_TR => {
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
            return;
        }
        overlay::CHIP_EDIT_EXP => {
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
            return;
        }
        
        overlay::CHIP_SWAP_LANG => {
            // 翻訳方向を反転 (en→ja ⇄ ja→en)
            if source.is_empty() {
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
            let pc = prompt_ctx_from_app(&source);
            crate::worker::retranslate(new_gen, cur_tr, cfg2, source, main, recog_id, pc);
            return;
        }
        overlay::CHIP_EXPLAIN_QUICK => {
            // 解説(即時): 編集ダイアログを出さず、既定プロンプトをそのまま送信する
            with_app(|app| {
                app.mode = Mode::Pinned;
                sync_overlay(app);
            });
            let Some(r_id) = recog_id else { return };
            if source.is_empty() {
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
            let pc = prompt_ctx_from_app(&source);
            let prompt = crate::worker::build_explain_prompt(&cfg, &pc).unwrap_or_default();
            if prompt.is_empty() {
                with_app(|app| {
                    app.badge = Some("LLM APIが設定されていません".into());
                    sync_overlay(app);
                });
                return;
            }
            let profile = cfg.active_api_profile.clone();
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
            crate::worker::explain(new_gen, r_id, cfg2, prompt, profile, main);
            return;
        }
        overlay::CHIP_EXPLAIN => {
            // プロンプト編集ウィンドウ (モードB) を表示してから送信 (SPECv0.4.7 §2)
            with_app(|app| {
                app.mode = Mode::Pinned;
                sync_overlay(app);
            });
            if let Some(r_id) = recog_id {
                if !source.is_empty() {
                    let dialog_pos = with_app(|app| {
                        let mut r = RECT::default();
                        unsafe {
                            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(app.overlay, &mut r);
                        }
                        (r.left + 24, r.top + 24)
                    })
                    .unwrap_or_default();

                    // 全プロファイルの解説テンプレートを渡す (テンプレート/送信内容の分離編集)
                    let profiles: Vec<crate::prompt_edit::ProfilePrompt> = cfg
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
                    let active_idx = cfg
                        .api_profiles
                        .iter()
                        .position(|p| p.name == cfg.active_api_profile)
                        .unwrap_or(0);

                    let pc = prompt_ctx_from_app(&source);
                    let main_hwnd_val = main;
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
        let mut keys = Vec::new();
        for k in engine::OCR_KEYS.iter() {
            if *k != "llm" {
                keys.push(k.to_string());
            }
        }
        for prof in cfg.ready_api_profiles() {
            keys.push(prof.name.clone());
        }
        let idx = id - overlay::CHIP_OCR_BASE;
        if idx >= keys.len() {
            return;
        }
        let selected_key = keys[idx].clone();
        let is_fixed = engine::OCR_KEYS.iter().any(|k| *k == selected_key && *k != "llm");
        let target_ocr = if is_fixed { selected_key.clone() } else { "llm".to_string() };

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
            return;
        }
        if is_fixed && !ensure_consent(&selected_key, &cfg) {
            return;
        }
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
        crate::worker::reocr(
            new_gen, capture_id, last_img, last_focus, origin.x, origin.y, target, target_ocr, cur_tr, cfg2,
            main, anchor, held_app_title, held_app_exe, held_uia_path, held_uia_nodes, held_control_type,
            held_selected_text,
        );
    } else if id >= overlay::CHIP_TR_BASE && id < overlay::CHIP_COPY {
        // 翻訳エンジン切替: 現在の原文を選択エンジンで再翻訳 (SPEC §8)
        let mut keys = Vec::new();
        for k in engine::TR_KEYS.iter() {
            if *k != "llm" {
                keys.push(k.to_string());
            }
        }
        for prof in cfg.ready_api_profiles() {
            keys.push(prof.name.clone());
        }
        let idx = id - overlay::CHIP_TR_BASE;
        if idx >= keys.len() {
            return;
        }
        let selected_key = keys[idx].clone();
        let is_fixed = engine::TR_KEYS.iter().any(|k| *k == selected_key && *k != "llm");
        let target_tr = if is_fixed { selected_key.clone() } else { "llm".to_string() };

        if source.is_empty() {
            return;
        }
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
            return;
        }
        if is_fixed && !ensure_consent(&selected_key, &cfg) {
            return;
        }

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
        let pc = prompt_ctx_from_app(&source);
        crate::worker::retranslate(new_gen, target_tr, cfg2, source, main, recog_id, pc);
    } else if id == overlay::CHIP_SELECTED_TEXT {
        // 選択中の文字列を(再)採用: 検出済みの選択テキストを原文として採用し再翻訳
        let Some(text) = with_app(|app| app.selected_text.clone())
            .flatten()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        adopt_text_and_retranslate(text, cur_tr, recog_id, main);
    } else if id >= overlay::CHIP_UIA_NODE_BASE {
        // UIAパスノード選択: そのノードのテキストを原文として採用し再翻訳
        let idx = id - overlay::CHIP_UIA_NODE_BASE;
        let Some(text) = with_app(|app| app.uia_nodes.get(idx).map(|n| n.text.trim().to_string()))
            .flatten()
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        adopt_text_and_retranslate(text, cur_tr, recog_id, main);
    }
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
