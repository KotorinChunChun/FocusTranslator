// チップ操作ハンドラ (SPEC v0.3 §8)
// オーバーレイのチップボタン押下時の処理をまとめたモジュール。
// app_state.rs から分離して可読性を向上させる。
use crate::app_state::{self, Mode, with_app, main_hwnd, sync_overlay};
use crate::config::Config;
use crate::engine;
use crate::overlay;
use crate::settings;
use crate::util;

use windows::Win32::Foundation::RECT;
use windows::core::w;

/// チップ押下 (SPEC §8): 押下時点でピン留めし、再OCR/再翻訳を実行
pub fn handle_chip(id: usize) {
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
        )
    }) else {
        return;
    };
    let (cfg, source, last_img, last_focus, origin, target, main, anchor, cur_tr, recog_id) = info;

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
        overlay::CHIP_IMAGE => {
            let img = with_app(|app| app.last_img.clone()).flatten();
            if let Some(i) = img {
                let png = crate::capture::to_png(&i);
                let path = crate::logdb::logs_dir().join("preview.png");
                if std::fs::write(&path, &png).is_ok() {
                    unsafe {
                        let wide = util::to_wide(&path.to_string_lossy());
                        let _ = windows::Win32::UI::Shell::ShellExecuteW(
                            None,
                            windows::core::w!("open"),
                            windows::core::PCWSTR(wide.as_ptr()),
                            windows::core::PCWSTR::null(),
                            windows::core::PCWSTR::null(),
                            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
                        );
                    }
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
            crate::worker::retranslate(new_gen, cur_tr, cfg2, source, main, recog_id);
            return;
        }
        overlay::CHIP_EXPLAIN_QUICK => {
            // 解説(即時): 編集ダイアログを出さず、既定プロンプトをそのまま送信する
            with_app(|app| {
                app.mode = Mode::Pinned;
                sync_overlay(app);
            });
            let Some(r_id) = recog_id else { return };
            let text = overlay::current_text().1.unwrap_or(source.clone());
            if text.is_empty() {
                return;
            }
            // キャッシュ済みの解説があれば即表示する
            if let Some(expl) = crate::logdb::get_explanation(r_id) {
                with_app(|app| {
                    app.mode = Mode::Pinned;
                    app.status = None;
                    app.error_only = false;
                    app.explanation = Some(expl);
                    sync_overlay(app);
                });
                return;
            }
            let (app_title, uia_path) =
                with_app(|app| (app.app_title.clone(), app.uia_path.clone())).unwrap_or_default();
            let prompt =
                crate::worker::build_explain_prompt(&cfg, &text, &app_title, &uia_path).unwrap_or_default();
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
            // プロンプト編集ダイアログを表示してから送信
            with_app(|app| {
                app.mode = Mode::Pinned;
                sync_overlay(app);
            });
            if let Some(r_id) = recog_id {
                let text = overlay::current_text().1.unwrap_or(source.clone());
                if !text.is_empty() {
                    let (app_title, uia_path, dialog_pos) = with_app(|app| {
                        let mut r = RECT::default();
                        unsafe {
                            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(app.overlay, &mut r);
                        }
                        (app.app_title.clone(), app.uia_path.clone(), (r.left + 24, r.top + 24))
                    })
                    .unwrap_or_default();

                    let initial_prompt =
                        crate::worker::build_explain_prompt(&cfg, &text, &app_title, &uia_path).unwrap_or_default();
                    if initial_prompt.is_empty() {
                        with_app(|app| {
                            app.badge = Some("LLM APIが設定されていません".into());
                            sync_overlay(app);
                        });
                        return;
                    }

                    let profiles: Vec<String> = cfg.api_profiles.iter().map(|p| p.name.clone()).collect();
                    let active_idx = cfg
                        .api_profiles
                        .iter()
                        .position(|p| p.name == cfg.active_api_profile)
                        .unwrap_or(0);

                    let main_hwnd_val = main;
                    let inst = with_app(|app| app.instance).unwrap_or_default();
                    crate::prompt_edit::open(
                        inst,
                        main_hwnd(),
                        Some(dialog_pos),
                        &initial_prompt,
                        profiles,
                        active_idx,
                        move |edited_prompt, profile| {
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
                        },
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

    if id < overlay::CHIP_OCR_BASE + engine::OCR_KEYS.len() {
        // OCRエンジン切替: 保持画像で再認識→現行エンジンで再翻訳 (SPEC §8)
        let key = engine::OCR_KEYS[id - overlay::CHIP_OCR_BASE].to_string();
        with_app(|app| { app.failed_ocr.remove(&key); });
        if !cfg.engine_available(&key) {
            with_app(|app| {
                app.status = Some(engine_unavailable_msg(&key));
                sync_overlay(app);
            });
            return;
        }
        if !ensure_consent(&key, &cfg) {
            return;
        }
        let new_gen = with_app(|app| {
            app.generation += 1;
            app.mode = Mode::Pinned;
            app.cur_ocr = key.clone();
            app.status = Some("再認識中…".into());
            app.busy = true;
            sync_overlay(app);
            app.generation
        })
        .unwrap_or(0);
        let cfg2 = Config::load();
        crate::worker::reocr(
            new_gen, last_img, last_focus, origin.x, origin.y, target, key, cur_tr, cfg2, main, anchor,
        );
    } else if id >= overlay::CHIP_TR_BASE && id < overlay::CHIP_TR_BASE + engine::TR_KEYS.len() {
        // 翻訳エンジン切替: 現在の原文を選択エンジンで再翻訳 (SPEC §8)
        let key = engine::TR_KEYS[id - overlay::CHIP_TR_BASE].to_string();
        with_app(|app| { app.failed_tr.remove(&key); });
        if source.is_empty() {
            return;
        }
        if !cfg.engine_available(&key) {
            with_app(|app| {
                app.status = Some(engine_unavailable_msg(&key));
                sync_overlay(app);
            });
            return;
        }
        if !ensure_consent(&key, &cfg) {
            return;
        }
        let new_gen = with_app(|app| {
            app.generation += 1;
            app.mode = Mode::Pinned;
            app.cur_tr = key.clone();
            let engine_name = engine::tr_display_name(&key, &app.cfg);
            app.status = Some(format!("{} で翻訳中…", engine_name));
            app.translation = None;
            app.busy = true;
            sync_overlay(app);
            app.generation
        })
        .unwrap_or(0);
        let cfg2 = Config::load();
        crate::worker::retranslate(new_gen, key, cfg2, source, main, recog_id);
    } else if id >= overlay::CHIP_UIA_NODE_BASE {
        // UIAパスノード選択: そのノードのテキストを原文として採用し再翻訳
        let idx = id - overlay::CHIP_UIA_NODE_BASE;
        let Some(text) = with_app(|app| app.uia_nodes.get(idx).map(|n| n.text.trim().to_string()))
            .flatten()
            .filter(|t| !t.is_empty())
        else {
            return;
        };
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
        crate::worker::retranslate(new_gen, cur_tr, cfg2, text, main, recog_id);
    }
}

fn engine_unavailable_msg(key: &str) -> String {
    match key {
        "paddle" => "PaddleOCRのモデルが未導入です。設定画面からインストールしてください".into(),
        "local" => "ローカル翻訳モデルが未導入です。設定画面からインストールしてください".into(),
        "yomitoku" | "ndl" => {
            "サーバーURLが未設定です。設定画面で接続テストを実行してください".into()
        }
        _ => "APIキーが未設定です。設定画面で設定してください".into(),
    }
}

/// 初回同意ダイアログ (SPEC §9.2)。同意済みなら true。
fn ensure_consent(engine_key: &str, cfg: &Config) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_YESNO, MessageBoxW};
    let (need, kind): (bool, &str) = match engine_key {
        "deepl" | "google" => (!cfg.consent_text, "text"),
        "llm" => (!cfg.consent_image || !cfg.consent_text, "image"),
        "yomitoku" => {
            if settings::is_localhost(&cfg.yomitoku_url) {
                (false, "ext")
            } else {
                (!cfg.consent_ext_ocr, "ext")
            }
        }
        "ndl" => {
            if settings::is_localhost(&cfg.ndl_url) {
                (false, "ext")
            } else {
                (!cfg.consent_ext_ocr, "ext")
            }
        }
        _ => (false, ""),
    };
    if !need {
        return true;
    }
    let msg = match kind {
        "text" => w!(
            "このエンジンはOCR済みの原文テキストを外部サービスへ送信します。\n初回のみ確認しています。許可しますか?"
        ),
        "image" => w!(
            "このエンジンはキャプチャ画像と言語設定を外部サービスへ送信する可能性があります。\n初回のみ確認しています。許可しますか?"
        ),
        _ => w!(
            "このエンジンは設定されたサーバーURLへキャプチャ画像を送信します。\n初回のみ確認しています。許可しますか?"
        ),
    };
    let r = unsafe { MessageBoxW(Some(main_hwnd()), msg, w!("外部送信の同意"), MB_YESNO) };
    if r == IDYES {
        let mut c = Config::load();
        match kind {
            "text" => c.consent_text = true,
            "image" => {
                c.consent_image = true;
                c.consent_text = true;
            }
            _ => c.consent_ext_ocr = true,
        }
        c.save();
        with_app(|app| app.cfg = c);
        true
    } else {
        false
    }
}
