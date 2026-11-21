use super::{ensure_ocr_consent, ensure_tr_consent, inspect_hold_target, prepare_ocr_input};
use crate::capture::{self, Captured};
use crate::config::{ApiProfile, ApiType, Config, PromptContext};
use crate::{capture_plan, ocr, translate, uia};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::RECT;

const SOURCE: &str = "Open settings";
const TRANSLATION: &str = "設定を開く";
const ENDPOINT: &str = "/v1/chat/completions";
const TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn recognition_translation_and_exclusion() {
    // ureqは環境のプロキシを参照する。ループバック以外へ試験データを送らない。
    assert!(
        ureq::Proxy::try_from_env().is_none()
            || std::env::var("NO_PROXY").is_ok_and(|value| {
                value
                    .split(',')
                    .any(|host| matches!(host.trim(), "127.0.0.1" | "*"))
            }),
        "プロキシを使う環境では NO_PROXY に 127.0.0.1 を設定してください"
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("ループバックの待受を開始");
    listener.set_nonblocking(true).unwrap();
    let mock = format!("http://{}{ENDPOINT}", listener.local_addr().unwrap());
    let cfg = Config {
        active_api_profile: "recognition-cli".into(),
        api_profiles: vec![ApiProfile {
            name: "recognition-cli".into(),
            api_type: ApiType::OpenAI,
            api_url: mock,
            model_name: "test-model".into(),
            ocr_prompt: "OCR {{app_exe}}".into(),
            translate_prompt: "{{source_lang}} -> {{target_lang}}: {{original_text}}".into(),
            ..Default::default()
        }],
        default_ocr: "llm".into(),
        default_translator: "llm".into(),
        disabled_apps: vec!["SampleViewer.exe".into()],
        ..Config::default()
    };
    let server = std::thread::spawn(move || {
        let request = respond(&listener, &json!({ "source": SOURCE }).to_string());
        let content = &request["messages"][0]["content"];
        assert_eq!(request["response_format"]["type"], "json_object");
        assert_eq!(content[0]["text"], "OCR AllowedViewer.exe");
        let sent = content[1]["image_url"]["url"].as_str().unwrap();
        let expected = STANDARD.encode(capture::to_png(&test_image(34..126, 68..132)));
        assert!(
            sent.strip_prefix("data:image/png;base64,") == Some(expected.as_str()),
            "OCRに送信された画像が期待する切り出し範囲と異なります"
        );

        let request = respond(&listener, TRANSLATION);
        assert_eq!(request["messages"][0]["content"], "en -> ja: Open settings");
        listener
    });

    let run = |exe: Option<String>| {
        let probe = uia::UiaProbe {
            hover_rect: Some(RECT {
                left: -260,
                top: 250,
                right: -180,
                bottom: 350,
            }),
            element_rect: None,
            line_rects: Vec::new(),
            node: String::new(),
            text: None,
            hover_text: None,
            selected_text: None,
        };
        let win = RECT {
            left: -300,
            top: 200,
            right: -100,
            bottom: 400,
        };
        let (rect, kind) = capture_plan::plan_capture_rect(&probe, &win, -230, 300);
        assert!(kind == capture_plan::CapKind::Hover);
        assert_eq!(
            rect,
            RECT {
                left: -266,
                top: 244,
                right: -174,
                bottom: 356
            }
        );
        let band =
            capture_plan::crop_screen_rect(test_image(0..200, 0..200), &win, &rect, 300).unwrap();
        assert_eq!(
            band.rect,
            RECT {
                left: 34,
                top: 44,
                right: 126,
                bottom: 156
            }
        );
        let context = PromptContext {
            app_exe: exe.unwrap(),
            ..Default::default()
        };
        let engine = &cfg.default_ocr;
        ensure_ocr_consent(&cfg, engine, 0).unwrap();
        let focus = capture_plan::focus_for(kind, band.focus_y);
        let (image, focus, full) = prepare_ocr_input(&cfg, engine, &band.img, focus, None);
        assert_eq!(focus, ocr::Focus::Line(56.0));
        let out = ocr::run(engine, &cfg, &image, focus, &context, full).unwrap();
        assert_eq!(out.text, SOURCE);
        assert!(out.translation.is_none());
        assert!(translate::is_source_lang_text(&cfg.source_lang, &out.text));
        let engine = &cfg.default_translator;
        ensure_tr_consent(&cfg, engine, 0, false).unwrap();
        translate::translate(engine, &cfg, &out.text, &context, None, None).unwrap()
    };

    let result = inspect_hold_target(&cfg, Some("AllowedViewer.exe".into()), run).unwrap();
    assert_eq!(result.text, TRANSLATION);
    assert!(!result.cache_hit);
    let listener = server.join().unwrap();
    let excluded = inspect_hold_target(&cfg, Some("SAMPLEVIEWER.EXE".into()), |_| {
        panic!("除外アプリの認識・翻訳処理が呼ばれました");
    });
    assert_eq!(excluded, None::<()>);
    assert!(matches!(listener.accept(), Err(err) if err.kind() == ErrorKind::WouldBlock));
    println!("認識範囲 92×64 px / 原文 {SOURCE} / 訳文 {TRANSLATION} / 除外時リクエスト 0 件");
}

/// 座標ごとに色が異なる画像。寸法だけでなく切り出し位置も照合できる。
fn test_image(x: std::ops::Range<u8>, y: std::ops::Range<u8>) -> Captured {
    let (width, height) = (x.len() as u32, y.len() as u32);
    let mut bgra = Vec::with_capacity((width * height * 4) as usize);
    for row in y {
        for col in x.clone() {
            bgra.extend_from_slice(&[col, row, 0, 255]);
        }
    }
    Captured {
        width,
        height,
        bgra,
    }
}

fn respond(listener: &TcpListener, response_text: &str) -> Value {
    let deadline = Instant::now() + TIMEOUT;
    let mut stream: TcpStream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(err) if err.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => panic!("モックにリクエストが届きませんでした: {err}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    stream.set_write_timeout(Some(TIMEOUT)).unwrap();
    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line.trim_end(), format!("POST {ENDPOINT} HTTP/1.1"));
    let mut length = None;
    loop {
        line.clear();
        assert_ne!(
            reader.read_line(&mut line).unwrap(),
            0,
            "HTTPヘッダーの途中で切断"
        );
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("Content-Length")
        {
            length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    let mut body = vec![0; length.expect("Content-Lengthがありません")];
    reader.read_exact(&mut body).unwrap();
    let response = json!({ "choices": [{ "message": { "content": response_text } }] }).to_string();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    ).unwrap();
    serde_json::from_slice(&body).unwrap()
}
