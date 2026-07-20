# FocusTranslator ビルド手順

本ドキュメントでは、開発者向けにFocusTranslatorのビルドおよびインストーラの作成手順を記載しています。

## インストーラのビルド

Inno Setup Compiler (`ISCC.exe`) を使用して独自のインストーラをビルドできます。

```bat
cargo build --release
"C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer.iss
```

生成物: `Output\focus-translator-setup.exe`
