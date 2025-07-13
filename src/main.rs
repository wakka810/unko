use std::{
    borrow::Cow,
    collections::{HashSet},
    env,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{ChildStdout, Command, Stdio},
};

use ansi_term::Colour::{Blue, Fixed, Green, Purple, Yellow};
use git2::Repository;
use once_cell::sync::Lazy;
use rayon::prelude::*;
use rustyline::{
    completion::{Completer, FilenameCompleter, Pair},
    config::{Builder as ConfigBuilder, CompletionType, Config, EditMode},
    error::ReadlineError,
    highlight::{Highlighter, MatchingBracketHighlighter},
    hint::Hinter,
    history::FileHistory,
    validate::{MatchingBracketValidator, Validator},
    Context, Editor, Helper,
};

static BIN_CACHE: Lazy<Vec<String>> = Lazy::new(|| {
    let mut bins = if let Some(path_var) = env::var_os("PATH") {
        env::split_paths(&path_var)
            .par_bridge()
            .map(|dir| {
                fs::read_dir(dir)
                    .map(|entries| entries.filter_map(Result::ok).collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_executable(p))
            .filter_map(|p| p.file_name().and_then(|n| n.to_str().map(String::from)))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    bins.sort();
    bins
});

#[derive(Debug, Default)]
struct CommandInfo {
    args: Vec<String>,
    stdin_path: Option<PathBuf>,
    stdout_path: Option<(PathBuf, bool)>, // (path, is_append)
    stderr_path: Option<PathBuf>,
}

struct ShellHelper {
    completer: FilenameCompleter,
    highlighter: MatchingBracketHighlighter,
    validator: MatchingBracketValidator,
    history: Vec<String>,
}

impl Helper for ShellHelper {}

impl Completer for ShellHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let (start, word) = extract_current_token(line, pos);

        if word.is_empty() {
            let mut out = Vec::new();
            for &b in ["echo", "ls", "cd", "pwd", "exit", "quit"].iter() {
                out.push(Pair {
                    display: b.into(),
                    replacement: b.into(),
                });
            }
            return Ok((start, out));
        }

        if !is_first_token(line, pos) {
            return self.completer.complete(line, pos, ctx);
        }

        if word.contains('/') || word.starts_with('.') {
            return self.completer.complete(line, pos, ctx);
        }

        let mut out = Vec::new();
        for &b in ["echo", "ls", "cd", "pwd", "exit", "quit"].iter() {
            if b.starts_with(word) {
                out.push(Pair {
                    display: b.into(),
                    replacement: b.into(),
                });
            }
        }
        for bin in BIN_CACHE.iter() {
            if bin.starts_with(word) {
                out.push(Pair {
                    display: bin.clone(),
                    replacement: bin.clone(),
                });
            }
        }

        Ok((start, out))
    }
}

fn is_first_token(line: &str, pos: usize) -> bool {
    !line[..pos].contains(char::is_whitespace)
}
fn extract_current_token(line: &str, pos: usize) -> (usize, &str) {
    let start = line[..pos]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + 1);
    (start, &line[start..pos])
}
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let ok = fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false);
    if !ok || !path.is_file() {
        return false;
    }
    const BAD: &[&str] = &["dll", "exe", "com"];
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if BAD.contains(&ext.to_ascii_lowercase().as_str()) {
            return false;
        }
    }
    true
}

impl Hinter for ShellHelper {
    type Hint = String;
    fn hint(&self, line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
        self.history
            .iter()
            .rev()
            .find(|h| h.starts_with(line) && h.len() > line.len())
            .map(|h| Fixed(8).paint(&h[line.len()..]).to_string())
    }
}

impl Highlighter for ShellHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        let mut out = String::with_capacity(line.len());
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\'' => {
                    out.push_str(&Yellow.paint("'").to_string());
                    while let Some(&n) = chars.peek() {
                        out.push_str(&Yellow.paint(n.to_string()).to_string());
                        chars.next();
                        if n == '\'' {
                            break;
                        }
                    }
                }
                '"' => {
                    out.push_str(&Purple.paint("\"").to_string());
                    while let Some(&n) = chars.peek() {
                        out.push_str(&Purple.paint(n.to_string()).to_string());
                        chars.next();
                        if n == '"' {
                            break;
                        }
                    }
                }
                '-' if out.ends_with(' ') || out.is_empty() => {
                    out.push_str(&Blue.paint("-").to_string());
                    while let Some(&n) = chars.peek() {
                        if n.is_whitespace() {
                            break;
                        }
                        out.push_str(&Blue.paint(n.to_string()).to_string());
                        chars.next();
                    }
                }
                _ => out.push(c),
            }
        }
        Cow::Owned(out)
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        Cow::Borrowed(prompt)
    }
}

impl Validator for ShellHelper {
    fn validate(
        &self,
        ctx: &mut rustyline::validate::ValidationContext<'_>,
    ) -> rustyline::Result<rustyline::validate::ValidationResult> {
        self.validator.validate(ctx)
    }
    fn validate_while_typing(&self) -> bool {
        self.validator.validate_while_typing()
    }
}

fn build_prompt() -> String {
    let user = env::var("USER").unwrap_or_default();
    let cwd = env::current_dir().unwrap_or_default();
    let path_display = if let Some(home) = dirs::home_dir() {
        if let Ok(p) = cwd.strip_prefix(&home) {
            if p.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", p.display())
            }
        } else {
            cwd.display().to_string()
        }
    } else {
        cwd.display().to_string()
    };
    let branch = Repository::discover(&cwd)
        .ok()
        .and_then(|repo| {
            repo.head()
                .ok()
                .and_then(|h| h.shorthand().map(|s| s.to_owned()))
        })
        .unwrap_or_default();
    let git_str = if branch.is_empty() {
        String::new()
    } else {
        format!(" {}", Purple.paint(format!("({})", branch)))
    };
    format!(
        "{}:{}{}{} ",
        Green.paint(user),
        Blue.paint(path_display),
        git_str,
        Blue.paint(">"),
    )
}

fn expand_var<I: Iterator<Item = char>>(iter: &mut std::iter::Peekable<I>) -> String {
    if let Some('{') = iter.peek().copied() {
        iter.next();
        let mut name = String::new();
        while let Some(&c) = iter.peek() {
            if c == '}' {
                iter.next();
                break;
            }
            name.push(c);
            iter.next();
        }
        env::var(name).unwrap_or_default()
    } else {
        let mut name = String::new();
        while let Some(&c) = iter.peek() {
            if c.is_alphanumeric() || c == '_' {
                name.push(c);
                iter.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            "$".to_string()
        } else {
            env::var(name).unwrap_or_default()
        }
    }
}

fn parse_commands(tokens: &[String]) -> Result<Vec<CommandInfo>, String> {
    let mut commands = Vec::new();
    if tokens.is_empty() {
        return Ok(commands);
    }

    for group in tokens.split(|token| token == "|") {
        if group.is_empty() {
            return Err("構文エラー: パイプの前後にはコマンドが必要です。".to_string());
        }

        let mut cmd_info = CommandInfo::default();
        let mut it = group.iter();
        while let Some(token) = it.next() {
            match token.as_str() {
                "<" => {
                    if let Some(path) = it.next() {
                        cmd_info.stdin_path = Some(PathBuf::from(path));
                    } else {
                        return Err("構文エラー: `<` の後にはファイル名が必要です。".to_string());
                    }
                }
                ">" => {
                    if let Some(path) = it.next() {
                        cmd_info.stdout_path = Some((PathBuf::from(path), false));
                    } else {
                        return Err("構文エラー: `>` の後にはファイル名が必要です。".to_string());
                    }
                }
                ">>" => {
                    if let Some(path) = it.next() {
                        cmd_info.stdout_path = Some((PathBuf::from(path), true));
                    } else {
                        return Err("構文エラー: `>>` の後にはファイル名が必要です。".to_string());
                    }
                }
                "2>" => {
                    if let Some(path) = it.next() {
                        cmd_info.stderr_path = Some(PathBuf::from(path));
                    } else {
                        return Err("構文エラー: `2>` の後にはファイル名が必要です。".to_string());
                    }
                }
                _ => {
                    cmd_info.args.push(token.clone());
                }
            }
        }
        if cmd_info.args.is_empty() {
            return Err("構文エラー: 実行するコマンドがありません。".to_string());
        }
        commands.push(cmd_info);
    }
    Ok(commands)
}

fn run_pipeline(commands: Vec<CommandInfo>) -> i32 {
    if commands.is_empty() {
        return 0;
    }

    let last_idx = commands.len() - 1;
    let mut previous_stdout: Option<ChildStdout> = None;
    let mut children = Vec::new();

    for (idx, mut cmd_info) in commands.into_iter().enumerate() {
        if cmd_info.args.is_empty() {
            eprintln!("エラー: パイプラインに空のコマンドが含まれています。");
            return 1;
        }

        if let Some(p) = resolve_command_path(&cmd_info.args[0]) {
            cmd_info.args[0] = p;
        }

        let mut cmd = Command::new(&cmd_info.args[0]);
        cmd.args(&cmd_info.args[1..]);

        if let Some(stdin_pipe) = previous_stdout.take() {
            cmd.stdin(Stdio::from(stdin_pipe));
        } else if let Some(path) = cmd_info.stdin_path {
            match File::open(&path) {
                Ok(file) => {
                    cmd.stdin(Stdio::from(file));
                }
                Err(e) => {
                    eprintln!("入力ファイル '{}' を開けませんでした: {}", path.display(), e);
                    return 1;
                }
            }
        } else {
            cmd.stdin(Stdio::inherit());
        }

        if idx == last_idx {
            if let Some((path, append)) = cmd_info.stdout_path {
                match fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(!append)
                    .append(append)
                    .open(&path)
                {
                    Ok(file) => {
                        cmd.stdout(Stdio::from(file));
                    }
                    Err(e) => {
                        eprintln!("出力ファイル '{}' を開けませんでした: {}", path.display(), e);
                        return 1;
                    }
                }
            } else {
                cmd.stdout(Stdio::inherit());
            }
        } else {
            if cmd_info.stdout_path.is_some() {
                eprintln!("エラー: 出力リダイレクションはパイプラインの最後のコマンドでのみ許可されています。");
                return 1;
            }
            cmd.stdout(Stdio::piped());
        }

        if let Some(path) = cmd_info.stderr_path {
            match fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
            {
                Ok(file) => {
                    cmd.stderr(Stdio::from(file));
                }
                Err(e) => {
                    eprintln!("エラー出力ファイル '{}' を開けませんでした: {}", path.display(), e);
                    return 1;
                }
            }
        } else {
            cmd.stderr(Stdio::inherit());
        }

        match cmd.spawn() {
            Ok(mut child) => {
                previous_stdout = if idx != last_idx {
                    child.stdout.take()
                } else {
                    None
                };
                children.push(child);
            }
            Err(e) => {
                eprintln!("コマンド実行失敗: {}: {}", cmd_info.args[0], e);
                return 1;
            }
        }
    }

    let mut last_status = 0;
    for mut child in children {
        match child.wait() {
            Ok(status) => last_status = status.code().unwrap_or(1),
            Err(_) => last_status = 1,
        }
    }
    last_status
}

fn resolve_command_path(cmd: &str) -> Option<String> {
    if cmd.contains('/') {
        return None;
    }
    let path_var = env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(cmd);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

fn try_builtin_special(argv: &[String]) -> bool {
    match argv.first().map(String::as_str) {
        Some("cd") => {
            if let Some(path) = argv.get(1).map(String::as_str) {
                if let Err(e) = env::set_current_dir(path) {
                    eprintln!("cd: {e}");
                }
            } else {
                let home = dirs::home_dir().unwrap_or_else(|| {
                    eprintln!("cd: (ホームディレクトリが分から)ないです");
                    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
                if let Err(e) = env::set_current_dir(home) {
                    eprintln!("cd: {e}");
                }
            }
            true
        }
        Some("exit") | Some("quit") => {
            let code = argv.get(1).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            std::process::exit(code);
        }
        _ => false,
    }
}

fn parse_line(input: &str) -> Result<Vec<String>, String> {
    enum State {
        Normal,
        Single,
        Double,
    }

    let mut state = State::Normal;
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match state {
            State::Normal => match c {
                ' ' | '\t' | '\n' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                '\'' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    state = State::Single;
                    current.push(c);
                }
                '"' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    state = State::Double;
                    current.push(c);
                }
                '\\' => {
                    if let Some(n) = chars.next() {
                        current.push(n);
                    }
                }
                '$' => current.push_str(&expand_var(&mut chars)),
                '|' | '<' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    tokens.push(c.to_string());
                }
                '>' => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    if chars.peek() == Some(&'>') {
                        chars.next();
                        tokens.push(">>".to_string());
                    } else {
                        tokens.push(">".to_string());
                    }
                }
                '2' if chars.peek() == Some(&'>') => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                    chars.next(); // consume '>'
                    tokens.push("2>".to_string());
                }
                _ => current.push(c),
            },
            State::Single => {
                current.push(c);
                if c == '\'' {
                    tokens.push(std::mem::take(&mut current));
                    state = State::Normal;
                }
            }
            State::Double => {
                match c {
                    '\\' => {
                        if let Some(n) = chars.next() {
                           current.push(n);
                        }
                    },
                    '$' => current.push_str(&expand_var(&mut chars)),
                    '"' => {
                        current.push(c);
                        tokens.push(std::mem::take(&mut current));
                        state = State::Normal;
                    }
                    _ => current.push(c),
                }
            }
        }
    }

    if !current.is_empty() {
        tokens.push(std::mem::take(&mut current));
    }

    let home = env::var("HOME").unwrap_or_default();
    for t in tokens.iter_mut() {
        if t.starts_with('~') && (t.len() == 1 || t.as_bytes()[1] == b'/') {
            let rest = &t[1..];
            *t = format!("{}{}", home, rest);
        }
    }
    Ok(tokens)
}


fn main() -> rustyline::Result<()> {
    let config: Config = ConfigBuilder::new()
        .history_ignore_dups(true)?
        .completion_type(CompletionType::List)
        .edit_mode(EditMode::Emacs)
        .build();

    let mut rl: Editor<ShellHelper, FileHistory> = Editor::with_config(config)?;
    rl.set_helper(Some(ShellHelper {
        completer: FilenameCompleter::new(),
        highlighter: MatchingBracketHighlighter::new(),
        validator: MatchingBracketValidator::new(),
        history: Vec::new(),
    }));

    let hist_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".unko_history");
    let _ = rl.load_history(&hist_path);

    let mut last_status = 0;

    loop {
        let mut full_input = String::new();
        let mut prompt = build_prompt();

        loop {
            match rl.readline(&prompt) {
                Ok(line) => {
                    if full_input.is_empty() && line.trim().is_empty() {
                        continue;
                    }

                    if line.ends_with('\\') {
                        let mut part = line.trim_end_matches('\\').trim_end().to_string();
                        if !full_input.trim_end().ends_with('|')
                            && !part.trim_start().starts_with('|')
                            && !full_input.is_empty()
                        {
                            full_input.push(' ');
                        }

                        full_input.push_str(part.trim_start());

                        prompt = "> ".into();
                        continue;
                    } else {
                        let part = line.trim_end();
                        if !full_input.trim_end().ends_with('|')
                            && !part.trim_start().starts_with('|')
                            && !full_input.is_empty()
                        {
                            full_input.push(' ');
                        }
                        full_input.push_str(part.trim_start());
                        break;
                    }
                }

                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    last_status = 130;
                    full_input.clear();
                    break;
                }
                Err(ReadlineError::Eof) => {
                    println!();
                    return Ok(());
                }
                Err(err) => {
                    eprintln!("これもうわかんねぇな…: {err}");
                    return Ok(());
                }
            }
        }

        let trimmed = full_input.trim();
        if trimmed.is_empty() {
            continue;
        }

        rl.add_history_entry(trimmed)?;
        rl.helper_mut().unwrap().history.push(trimmed.to_owned());

        match parse_line(trimmed) {
            Ok(tokens) if tokens.is_empty() => continue,
            Ok(tokens) => {
                let first_cmd = tokens.first().map(String::as_str).unwrap_or("");
                if first_cmd == "cd" || first_cmd == "exit" || first_cmd == "quit" {
                    if tokens.contains(&"|".to_string()) {
                        eprintln!("エラー: '{}' はパイプラインでは使用できません。", first_cmd);
                        last_status = 1;
                        continue;
                    }
                    if tokens.iter().any(|t| t == ">" || t == ">>" || t == "<" || t == "2>") {
                        eprintln!("エラー: '{}' はリダイレクションをサポートしていません。", first_cmd);
                        last_status = 1;
                        continue;
                    }
                    try_builtin_special(&tokens);
                    last_status = 0;
                } else {
                    match parse_commands(&tokens) {
                        Ok(pipeline) => {
                            last_status = run_pipeline(pipeline);
                        }
                        Err(e) => {
                            eprintln!("エラー: {}", e);
                            last_status = 1;
                        }
                    }
                }
            }
            Err(e) => eprintln!("{e}"),
        }
    }
}