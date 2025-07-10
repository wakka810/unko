use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    env,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

use ansi_term::Colour::{Blue, Fixed, Green, Purple, Red, Yellow};
use git2::Repository;
use once_cell::sync::Lazy;
use regex::Regex;
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

static ARG_CACHE: Lazy<Mutex<HashMap<String, Vec<String>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static FLAG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"-{1,2}[A-Za-z0-9][A-Za-z0-9_-]*").unwrap());
static SUB_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s{2,}([A-Za-z0-9][A-Za-z0-9_-]+)\s").unwrap());

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

        if is_first_token(line, pos) {
            let mut cand = Vec::new();
            let mut seen = HashSet::new();

            for &b in ["echo", "ls", "cd", "pwd", "exit", "quit"].iter() {
                if b.starts_with(word) && seen.insert(b.into()) {
                    cand.push(Pair {
                        display: b.into(),
                        replacement: b.into(),
                    });
                }
            }
            if let Some(path_var) = env::var_os("PATH") {
                for dir in env::split_paths(&path_var) {
                    if let Ok(entries) = fs::read_dir(&dir) {
                        for e in entries.filter_map(Result::ok) {
                            let fname = e.file_name().to_string_lossy().into_owned();
                            if !fname.starts_with(word) || !is_executable(&e.path()) {
                                continue;
                            }
                            if seen.insert(fname.clone()) {
                                cand.push(Pair {
                                    display: fname.clone(),
                                    replacement: fname,
                                });
                            }
                        }
                    }
                }
            }
            return Ok((start, cand));
        }

        let tokens: Vec<&str> = line[..pos].split_whitespace().collect();
        if tokens.is_empty() {
            return self.completer.complete(line, pos, ctx);
        }
        let cmd = tokens[0];
        let subcmd = tokens.get(1).copied();

        let mut cand = Vec::new();
        for a in fetch_args(cmd, subcmd) {
            if a.starts_with(word) {
                cand.push(Pair {
                    display: a.clone(),
                    replacement: a,
                });
            }
        }

        let (f_start, mut f_cand) = self.completer.complete(line, pos, ctx)?;
        cand.extend(f_cand);
        Ok((start.min(f_start), cand))
    }
}

fn fetch_args(cmd: &str, subcmd: Option<&str>) -> Vec<String> {
    let key = subcmd.map_or_else(|| cmd.to_string(), |s| format!("{cmd} {s}"));

    if let Some(c) = ARG_CACHE.lock().unwrap().get(&key) {
        return c.clone();
    }

    let mut out = Vec::new();

    if subcmd.is_none() {
        out.extend(parse_subcommands(cmd));
    }

    out.extend(parse_flags(cmd, subcmd));

    out.sort();
    out.dedup();
    ARG_CACHE.lock().unwrap().insert(key, out.clone());
    out
}

fn parse_subcommands(cmd: &str) -> Vec<String> {
    Command::new(cmd)
        .arg("--help")
        .output()
        .ok()
        .map(|out| {
            SUB_RE
                .captures_iter(&String::from_utf8_lossy(&out.stdout))
                .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_flags(cmd: &str, subcmd: Option<&str>) -> Vec<String> {
    let mut c = Command::new(cmd);
    if let Some(sc) = subcmd {
        c.arg(sc);
    }
    c.arg("--help")
        .output()
        .ok()
        .map(|out| {
            FLAG_RE
                .find_iter(&String::from_utf8_lossy(&out.stdout))
                .map(|m| m.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
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

fn build_prompt(last_status: i32) -> String {
    let cwd = env::current_dir().unwrap_or_default();
    let path_display = cwd.display();
    let branch = Repository::discover(&cwd)
        .ok()
        .and_then(|repo| {
            repo.head()
                .ok()
                .and_then(|h| h.shorthand().map(|s| s.to_owned()))
        })
        .unwrap_or_default();
    let status_str = if last_status == 0 {
        Green.paint(format!("[{}]", last_status)).to_string()
    } else {
        Red.paint(format!("[{}]", last_status)).to_string()
    };
    let git_str = if branch.is_empty() {
        String::new()
    } else {
        format!(" {}", Purple.paint(format!("({})", branch)))
    };
    format!(
        "{}{}{} ",
        Blue.paint(path_display.to_string()),
        git_str,
        Blue.paint(">"),
        // status_str
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

fn parse_line(input: &str) -> Result<Vec<String>, String> {
    enum State {
        Normal,
        Single,
        Double,
    }
    let mut state = State::Normal;
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        match state {
            State::Normal => match c {
                ' ' | '\t' => {
                    if !current.is_empty() {
                        tokens.push(current.clone());
                        current.clear();
                    }
                }
                '\'' => state = State::Single,
                '"' => state = State::Double,
                '\\' => {
                    if let Some(n) = chars.next() {
                        current.push(n);
                    }
                }
                '$' => current.push_str(&expand_var(&mut chars)),
                _ => current.push(c),
            },
            State::Single => {
                if c == '\'' {
                    state = State::Normal;
                } else {
                    current.push(c);
                }
            }
            State::Double => match c {
                '"' => state = State::Normal,
                '\\' => {
                    if let Some(n) = chars.next() {
                        current.push(n);
                    }
                }
                '$' => current.push_str(&expand_var(&mut chars)),
                _ => current.push(c),
            },
        }
    }
    if !matches!(state, State::Normal) {
        return Err("これもうわかんねぇな…: unmatched quote".into());
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    let home = env::var("HOME").unwrap_or_default();
    for t in tokens.iter_mut() {
        if t.starts_with('~') && (t.len() == 1 || t.as_bytes()[1] == b'/') {
            let rest = &t[1..];
            *t = format!("{home}{rest}");
        }
    }
    Ok(tokens)
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

fn try_builtin(argv: &[String]) -> bool {
    match argv.first().map(String::as_str) {
        Some("echo") => {
            println!("{}", argv[1..].join(" "));
            true
        }
        Some("ls") => {
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
        Some("pwd") => {
            if let Ok(path) = env::current_dir() {
                println!("{}", path.display());
            } else {
                eprintln!("pwd: (カレントディレクトリが分から)ないです");
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

fn run_external(argv: &[String]) -> i32 {
    if argv.is_empty() {
        return 0;
    }
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    match status {
        Ok(s) => match s.code() {
            Some(0) => 0,
            Some(1) => {
                eprintln!("またのぅ～ (Exit: 1)");
                1
            }
            Some(code) => {
                eprintln!("ファッ！？ｳｰﾝ…: （コード{code}）");
                code
            }
            None => {
                eprintln!("んにゃぴ・・・");
                1
            }
        },
        Err(e) => {
            match e.raw_os_error() {
                Some(2) => eprintln!("知らねーよ、そんなの"),
                Some(13) => eprintln!("駄目です（権限なし）"),
                Some(code) => eprintln!("これもうわかんねぇな… {code}: {e}"),
                None => eprintln!("よくわかんなかったです(OSエラー): {e}"),
            }
            1
        }
    }
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
        let prompt = build_prompt(last_status);
        match rl.readline(&prompt) {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                rl.add_history_entry(trimmed)?;
                rl.helper_mut().unwrap().history.push(trimmed.to_owned());

                match parse_line(trimmed) {
                    Ok(argv) if argv.is_empty() => continue,
                    Ok(argv) => {
                        if try_builtin(&argv) {
                            last_status = 0;
                            continue;
                        }
                        let mut argv_exec = argv.clone();
                        if let Some(p) = resolve_command_path(&argv_exec[0]) {
                            argv_exec[0] = p;
                        }
                        last_status = run_external(&argv_exec);
                    }
                    Err(e) => eprintln!("{e}"),
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                last_status = 130;
            }
            Err(ReadlineError::Eof) => {
                println!();
                break;
            }
            Err(err) => {
                eprintln!("これもうわかんねぇな…: {err}");
                break;
            }
        }
    }
    let _ = rl.append_history(&hist_path);
    Ok(())
}