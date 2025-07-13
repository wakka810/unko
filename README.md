# unko

Rustで書かれたシンプルなUNIX風シェルです。

## 特徴
- REPL形式の対話型インターフェース
- プロンプトにユーザー名、カレントディレクトリ、Gitブランチを表示
- コマンド履歴の保存と読み込み (`~/.unko_history`)
- 組み込みコマンド: `cd`, `pwd`, `echo`, `exit`, `quit`, `ls`
- 外部コマンドの実行とPATH解決
- パイプ (`|`) によるコマンドの連結実行
- リダイレクション (`<`, `>`, `>>`, `2>`)
- ファイル名、コマンド名、引数（フラグとサブコマンド）のタブ補完
- 入力中のシンタックスハイライト
- 履歴に基づいたコマンド入力ヒント
- 変数展開 (`$VAR`, `${VAR}`)
- クォート (`'`, `"`) とエスケープ (`\`) の処理
- チルダ展開 (`~`)
- 複数行入力 (`\`)
- `Ctrl-C` による入力キャンセル
- 起動時の高速なコマンドキャッシュ

## 使い方

```
cargo run
```

## インストール

```bash
curl -sSL https://raw.githubusercontent.com/wakka810/unko/main/install.sh | bash
```

## アンインストール

```bash
curl -sSL https://raw.githubusercontent.com/wakka810/unko/main/uninstall.sh | bash
```

## ビルド

```
cargo build --release
```

## 依存クレート
- `ansi_term = "0.12.1"`
- `dirs = "6.0.0"`
- `fs = "0.0.5"`
- `git2 = "0.20.2"`
- `libc = "0.2.174"`
- `once_cell = "1.21.3"`
- `rayon = "1.10.0"`
- `rustyline = "16.0.0"`

## 今後の予定

- ジョブ管理
- スクリプト実行
- ユーザー設定ファイル
- その他
