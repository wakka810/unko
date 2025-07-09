use std::{
    io::{self, Write},
    process::{Command, Stdio},
};

/// 組み込みコマンドかを判定し、処理する
fn try_builtin(argv: &[String]) -> bool {
    match argv.first().map(String::as_str) {
        Some("echo") => {
            // 標準出力にそのまま書き出し
            println!("{}", argv[1..].join(" "));
            true
        }
        Some("ls") => {
            // 引数があればそのパス、無ければカレントディレクトリ
            let path = argv.get(1).map(String::as_str).unwrap_or(".");
            match std::fs::read_dir(path) {
                Ok(entries) => {
                    for entry in entries.filter_map(Result::ok) {
                        print!("{}  ", entry.file_name().to_string_lossy());
                    }
                    println!();
                }
                Err(e) => eprintln!("ls: {e}"),
            }
            true
        }
        Some("cd") => {
            // 引数があればそのパス、無ければホームディレクトリ
            if let Some(path) = argv.get(1).map(String::as_str) {
                if let Err(e) = std::env::set_current_dir(path) {
                    eprintln!("cd: {e}");
                }
            } else {
                let home = dirs::home_dir().unwrap_or_else(|| {
                    eprintln!("cd: (ホームディレクトリが分から)ないです");
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                });
                if let Err(e) = std::env::set_current_dir(home) {
                    eprintln!("cd: {e}");
                }
            }
            true
        }
        Some("pwd") => {
            // カレントディレクトリの表示
            if let Ok(path) = std::env::current_dir() {
                println!("{}", path.display());
            } else {
                eprintln!("pwd: (カレントディレクトリが分から)ないです");
            }
            true
        }
        Some("exit") | Some("quit") => {
            // ステータスコードを引数に終了（無ければ0）
            let code = argv.get(1).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            std::process::exit(code);
        }
        _ => false, // 組み込みではない
    }
}

/// 外部コマンドを実行
fn run_external(argv: &[String]) {
    if argv.is_empty() {
        return;
    }
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        // 標準入出力をそのまま継承
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    match status {
        Ok(s) => {
            match s.code() {
                Some(0) => {
                    // println!("終わり！閉廷！以上！皆解散！");
                }
                Some(1) => {
                    eprintln!("またのぅ～ (Exit: 1)");
                }
                Some(code) => {
                    eprintln!("ファッ！？ｳｰﾝ…: （コード{code}）");
                }
                None => {
                    eprintln!("んにゃぴ・・・");
                }
            }
        }
        Err(e) => {
            match e.raw_os_error() {
                Some(2) => eprintln!("知らねーよ、そんなの"),
                Some(13) => eprintln!("駄目です（権限なし）"),
                Some(code) => eprintln!("これもうわかんねぇな… {code}: {e}"),
                None => eprintln!("よくわかんなかったです(OSエラー): {e}"),
            }
        }
    }
}

fn main() {
    let mut line = String::new();
    loop {
        // プロンプト表示
        print!("unko> ");
        io::stdout().flush().unwrap();

        line.clear();
        // Ctrl-D で終了
        if io::stdin().read_line(&mut line).is_err() {
            println!();
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // 引数の安全な分割
        let argv: Vec<String> = match shell_words::split(trimmed) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => continue,
            Err(e) => {
                eprintln!("これもうわかんねぇな…: {e}");
                continue;
            }
        };

        // 組み込み → 外部 の順で試行
        if !try_builtin(&argv) {
            run_external(&argv);
        }
    }
}
