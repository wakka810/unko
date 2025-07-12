use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    env,
    fs::{self, File, OpenOptions},
    os::unix::io::AsRawFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

use ansi_term::Colour::{Blue, Fixed, Green, Purple, Red, Yellow};
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

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use signal_hook::{
    consts::{SIGCHLD, SIGINT},
    iterator::Signals,
};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::sys::signal::{kill, Signal};
use nix::unistd::{getpid, setpgid, Pid};
use std::os::unix::process::CommandExt;

static JOB_COUNTER: AtomicUsize = AtomicUsize::new(1);
static JOBS: Lazy<Mutex<HashMap<usize, Job>>> = Lazy::new(|| Mutex::new(HashMap::new()));

static SHELL_PGID: Lazy<Pid> = Lazy::new(|| getpid());
static TTY_FD: Lazy<i32> = Lazy::new(|| 0);

#[derive(Debug , Clone)]
struct Job {
    id: usize,
    pgid: Pid,
    pids: Vec<u32>,
    cmd: String,
}


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

struct ParsedCommand {
    argv: Vec<String>,
    stdin_path: Option<String>,
    stdout_path: Option<String>,
    stdout_append: bool,
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

        let mut is_command = true;
        if start > 0 {
            if let Some(prev_char) = line[..start].chars().rev().find(|c| !c.is_whitespace()) {
                if !matches!(prev_char, '|' | ';' | '&' | '(') {
                    is_command = false;
                }
            }
        }

        if is_command {
            if word.contains('/') || word.starts_with('.') {
                return self.completer.complete(line, pos, ctx);
            }

            let mut out = Vec::new();
            let builtins = ["echo", "ls", "cd", "pwd", "exit", "quit", "jobs", "fg", "bg"];
            for &b in builtins.iter() {
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
        } else {
            self.completer.complete(line, pos, ctx)
        }
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

fn build_prompt(last_status: i32) -> String {
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

fn parse_pipeline(input: &str) -> Result<Vec<Vec<String>>, String> {
    let mut commands = Vec::new();
    for command_str in input.split('|') {
        let tokens = parse_line(command_str.trim())?;
        if !tokens.is_empty() {
            commands.push(tokens);
        }
    }
    Ok(commands)
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
                '>' | '<' => {
                    if !current.is_empty() {
                        tokens.push(current.clone());
                        current.clear();
                    }
                    current.push(c);
                    if c == '>' && chars.peek() == Some(&'>') {
                        current.push(chars.next().unwrap());
                    }
                    tokens.push(current.clone());
                    current.clear();
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

fn process_redirections(tokens: Vec<String>) -> Result<ParsedCommand, String> {
    let mut argv = Vec::new();
    let mut stdin_path = None;
    let mut stdout_path = None;
    let mut stdout_append = false;
    let mut iter = tokens.into_iter().peekable();

    while let Some(token) = iter.next() {
        match token.as_str() {
            "<" => {
                if stdin_path.is_some() {
                    return Err("入力リダイレクトが複数あります".into());
                }
                stdin_path = iter.next();
                if stdin_path.is_none() {
                    return Err("入力リダイレクトの後にファイル名がありません".into());
                }
            }
            ">" => {
                if stdout_path.is_some() {
                    return Err("出力リダイレクトが複数あります".into());
                }
                stdout_path = iter.next();
                if stdout_path.is_none() {
                    return Err("出力リダイレクトの後にファイル名がありません".into());
                }
                stdout_append = false;
            }
            ">>" => {
                if stdout_path.is_some() {
                    return Err("出力リダイレクトが複数あります".into());
                }
                stdout_path = iter.next();
                if stdout_path.is_none() {
                    return Err("出力リダイレクトの後にファイル名がありません".into());
                }
                stdout_append = true;
            }
            _ => argv.push(token),
        }
    }

    Ok(ParsedCommand {
        argv,
        stdin_path,
        stdout_path,
        stdout_append,
    })
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
        Some("exit") | Some("quit") => {
            let code = argv.get(1).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            std::process::exit(code);
        }
        Some("jobs") => {
            let jobs = JOBS.lock().unwrap();
            for (id, job) in jobs.iter() {
                let pidlist = job.pids.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("[{}] 実行中  {}    {}", id, pidlist, job.cmd);
            }
            true
        }
        Some("fg") => {
            if let Some(arg) = argv.get(1) {
                let jid = arg.trim_start_matches('%').parse::<usize>().unwrap_or(0);
                if let Some(job) = JOBS.lock().unwrap().remove(&jid) {
                    unsafe { libc::tcsetpgrp(*TTY_FD, job.pgid.as_raw()) };
                    let _ = kill(Pid::from_raw(-job.pgid.as_raw()), Signal::SIGCONT);
                    let _ = waitpid(Pid::from_raw(-job.pgid.as_raw()), None);
                    unsafe { libc::tcsetpgrp(*TTY_FD, SHELL_PGID.as_raw()) };
                } else {
                    eprintln!("fg: ジョブ {} が見つかりません", jid);
                }
            } else {
                eprintln!("fg: ジョブ番号を指定してください");
            }
            true
        }
        Some("bg") => {
            if let Some(arg) = argv.get(1) {
                let jid = arg.trim_start_matches('%').parse::<usize>().unwrap_or(0);
                if let Some(job) = JOBS.lock().unwrap().get(&jid) {
                    let _ = kill(Pid::from_raw(-job.pgid.as_raw()), Signal::SIGCONT);
                    println!("[{}] {} をバックグラウンドで再開", jid, job.cmd);
                } else {
                    eprintln!("bg: ジョブ {} が見つかりません", jid);
                }
            } else {
                eprintln!("bg: ジョブ番号を指定してください");
            }
            true
        }
        _ => false,
    }
}

fn run_pipeline(commands: &[Vec<String>]) -> i32 {
    if commands.is_empty() {
        return 0;
    }

    let (mut children, pgid) = match spawn_pipeline(commands, false) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    if children.is_empty() {
        return 0;
    }

    unsafe {
        libc::tcsetpgrp(*TTY_FD, pgid.as_raw());
    }

    let mut status_code = 0;
    for child in &mut children {
        if let Ok(status) = child.wait() {
            status_code = status.code().unwrap_or(1);
        }
    }

    unsafe { libc::tcsetpgrp(*TTY_FD, SHELL_PGID.as_raw()) };

    status_code
}

fn run_external(argv: &[String]) -> i32 {
    if argv.is_empty() {
        return 0;
    }

    let (mut children, pgid) = match spawn_pipeline(&[argv.to_vec()], false) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    if children.is_empty() {
        return 0;
    }

    unsafe { libc::tcsetpgrp(*TTY_FD, pgid.as_raw()) };

    let mut status_code = 0;
    for child in &mut children {
        if let Ok(status) = child.wait() {
            status_code = status.code().unwrap_or(1);
        }
    }

    unsafe { libc::tcsetpgrp(*TTY_FD, SHELL_PGID.as_raw()) };
    status_code
}

/// すべてのシグナルハンドラをまとめて初期化
fn setup_signal_handlers() -> std::io::Result<()> {
    // SIGINT (^C) と SIGCHLD を捕捉
    let mut signals = Signals::new(&[SIGINT, SIGCHLD])?;
    std::thread::spawn(move || {
        for sig in signals.forever() {
            match sig {
                SIGINT => {
                    // readline が中断済みなので改行だけ差し込む
                    eprintln!();          // stdout へ出すとプロンプトが壊れる
                }
                SIGCHLD => reap_zombies(),
                _ => {}
            }
        }
    });
    Ok(())
}

/// 終了した子プロセスを回収して JOBS から削除
fn reap_zombies() {
    loop {
        match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::Exited(pid, status)) => {
                remove_job(pid.as_raw() as u32, status);
            }
            Ok(WaitStatus::Signaled(pid, sig, _)) => {
                remove_job(pid.as_raw() as u32, 128 + sig as i32);
            }
            Ok(WaitStatus::StillAlive) => break,
            Ok(_) => {}
            Err(nix::errno::Errno::ECHILD) => break,
            Err(e) => {
                eprintln!("waitpid 失敗: {e}");
                break;
            }
        }
    }
}

/// 終了したジョブを管理テーブルから外し、ユーザへ通知
fn remove_job(pid: u32, status: i32) {
    // ① 不変参照で見つけた (jid, Job) をクローンして取得
    let maybe: Option<(usize, Job)> = {
        let jobs_guard = JOBS.lock().unwrap();
        jobs_guard
            .iter()
            .find(|(_, job)| job.pids.contains(&pid))
            .map(|(&jid, job)| (jid, job.clone()))
    }; // ← ここで jobs_guard はドロップされる

    if let Some((jid, job)) = maybe {
        // ② 改めて可変ガードを取得して削除
        let mut jobs_guard = JOBS.lock().unwrap();
        jobs_guard.remove(&jid);
        println!("[{}] 終了 ({})  {}", jid, status, job.cmd);
    }
}

/// 末尾の '&' を判定して削除
fn split_background(input: &str) -> (String, bool) {
    let trimmed = input.trim_end();
    if trimmed.ends_with('&') {
        (
            trimmed[..trimmed.len() - 1].trim_end().to_string(),
            true,
        )
    } else {
        (input.to_string(), false)
    }
}

/// wait せずにパイプラインを起動し、子プロセス配列を返す
fn spawn_child(
    argv: &[String],
    pgid: Option<Pid>,
    stdin: Stdio,
    stdout: Stdio,
) -> std::io::Result<std::process::Child> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::inherit());

    unsafe {
        let pg = pgid;
        cmd.pre_exec(move || {
            let pid = getpid();
            let target = pg.unwrap_or(pid);
            setpgid(Pid::from_raw(0), target)?;
            Ok(())
        });
    }

    cmd.spawn()
}

/// wait せずにパイプラインを起動し、子プロセス配列と PGID を返す
fn spawn_pipeline(commands: &[Vec<String>], background: bool) -> Result<(Vec<std::process::Child>, Pid), String> {
    if commands.is_empty() {
        return Ok((vec![], Pid::from_raw(0)));
    }

    let mut children = Vec::new();
    let mut prev_stdout = Stdio::inherit();
    let mut pgid: Option<Pid> = None;

    for (i, raw_argv) in commands.iter().enumerate() {
        let parsed_cmd = process_redirections(raw_argv.clone())?;
        let mut argv = parsed_cmd.argv;
        if argv.is_empty() {
            continue;
        }
        if let Some(path) = resolve_command_path(&argv[0]) {
            argv[0] = path;
        }

        let is_first = i == 0;
        let is_last = i + 1 == commands.len();

        let stdin = if is_first {
            if let Some(p) = parsed_cmd.stdin_path {
                Stdio::from(File::open(p).map_err(|e| format!("open stdin: {e}"))?)
            } else if background {
                Stdio::null()
            } else {
                prev_stdout
            }
        } else {
            prev_stdout
        };

        let stdout = if is_last {
            if let Some(p) = parsed_cmd.stdout_path {
                let mut opt = OpenOptions::new();
                opt.write(true).create(true);
                if parsed_cmd.stdout_append {
                    opt.append(true);
                } else {
                    opt.truncate(true);
                }
                Stdio::from(opt.open(p).map_err(|e| format!("open stdout: {e}"))?)
            } else {
                Stdio::inherit()
            }
        } else {
            Stdio::piped()
        };

        let mut child = spawn_child(&argv, pgid, stdin, stdout)
            .map_err(|e| format!("spawn 失敗: {e}"))?;

        if pgid.is_none() {
            pgid = Some(Pid::from_raw(child.id() as i32));
        }

        prev_stdout = child
            .stdout
            .take()
            .map_or(Stdio::null(), |o| Stdio::from(o));

        children.push(child);
    }
    Ok((children, pgid.unwrap()))
}

/// '&' 付きで投入されたジョブをバックグラウンドで実行
fn run_pipeline_background(commands: &[Vec<String>]) -> Result<(), String> {
    let (children, pgid) = spawn_pipeline(commands, true)?;
    if children.is_empty() {
        return Ok(());
    }

    let job_id = JOB_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pids: Vec<u32> = children.iter().map(|c| c.id()).collect();

    let cmdline = commands
        .iter()
        .flat_map(|v| v.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");

    {
        let mut jobs = JOBS.lock().unwrap();
        jobs.insert(
            job_id,
            Job {
                id: job_id,
                pgid,
                pids: pids.clone(),
                cmd: cmdline.clone(),
            },
        );
    }

    // 子プロセスは signal handler が回収するので Drop は OK
    std::mem::drop(children);

    println!("[{job_id}] {}", pgid.as_raw());
    Ok(())
}


fn main() -> rustyline::Result<()> {
    setup_signal_handlers().expect("signal 初期化失敗");
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
                let (line, bg) = split_background(&line);
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                rl.add_history_entry(trimmed)?;
                rl.helper_mut().unwrap().history.push(trimmed.to_owned());

                match parse_pipeline(trimmed) {
                    Ok(commands) if commands.is_empty() => continue,
                    Ok(commands) => {
                        if bg {
                            // バックグラウンド
                            if let Err(e) = run_pipeline_background(&commands) {
                                eprintln!("{e}");
                                last_status = 1;
                            } else {
                                last_status = 0;
                            }
                            continue;
                        }

                        if commands.len() == 1 {
                            let argv = &commands[0];
                            if try_builtin(argv) {
                                last_status = 0;
                            } else {
                                last_status = run_external(argv);
                            }
                        } else {
                            last_status = run_pipeline(&commands);
                        }
                    }
                    Err(e) => {
                        eprintln!("{e}");
                        last_status = 1;
                    }
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