fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        // リソースID "1" として埋め込む。exeアイコン兼、実行時 LoadIconW(instance, 1) で参照する。
        res.set_icon("FocusTranslator_icon_32x32.ico");
        // タスクマネージャーやexeプロパティに出る表示名 (内部名は focus-translator のまま)
        res.set("ProductName", "なにこれ？（Focus Translator）");
        res.set("FileDescription", "なにこれ？（Focus Translator）");
        res.compile().expect("アイコンリソースの埋め込みに失敗しました");
    }
}
