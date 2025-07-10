# unko

Rustで書かれたシンプルなUNIX風シェルです。

## 特徴
- REPL形式の対話型インターフェース
- プロンプトにカレントディレクトリとGitブランチを表示
- コマンド履歴の保存と読み込み (`~/.unko_history`)
- 組み込みコマンド: `cd`, `pwd`, `echo`, `exit`, `quit`, `ls`
- 外部コマンドの実行とPATH解決
- ファイル名、コマンド名、引数（フラグとサブコマンド）のタブ補完
- 入力中のシンタックスハイライト
- 履歴に基づいたコマンド入力ヒント
- 変数展開 (`$VAR`, `${VAR}`)
- クォート (`'`, `"`) とエスケープ (`\`) の処理
- チルダ展開 (`~`)

## 使い方

```
cargo run
```

## インストール

```bash
curl -sSL https://raw.githubusercontent.com/wakka810/unko/day2/install.sh | bash
```

## アンインストール

```bash
curl -sSL https://raw.githubusercontent.com/wakka810/unko/day2/uninstall.sh | bash
```

## ビルド

```
cargo build --release
```

## 依存クレート
- `ansi_term`
- `dirs`
- `git2`
- `once_cell`
- `regex`
- `rustyline`

## 今後の予定

- パイプ・リダイレクト
- シグナル処理
- ジョブ管理
- スクリプト実行
- ユーザー設定ファイル
- その他