//! j — a functional-programming-centered CLI for jj repositories (§1).

use j::config::{self, Config};
use j::eval::Interp;
use j::jj::JjBackend;
use j::parse::parse_expr;
use j::value::{Crash, Value};
use std::collections::BTreeSet;
use std::io::{IsTerminal, Read};
use std::process::ExitCode;
use std::rc::Rc;

const USAGE: &str = "usage: j EXPRESSION…   (or echo EXPRESSION | j)";

fn err(status: u8, msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("j: {}", msg);
    ExitCode::from(status)
}

fn crash_err(c: &Crash, expr_text: Option<&str>) -> ExitCode {
    eprintln!("j: crash: {}", c.msg);
    match (&c.def, expr_text) {
        (Some(d), Some(e)) => eprintln!("   in {}, from {}", d, e),
        (None, Some(e)) => eprintln!("   from {}", e),
        (Some(d), None) => eprintln!("   in {}", d),
        (None, None) => {}
    }
    ExitCode::from(1)
}

fn config_path() -> String {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return format!("{}/j/config.j", xdg);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    format!("{}/.config/j/config.j", home)
}

fn load_config_file() -> Result<(Config, String), ExitCode> {
    let path = config_path();
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => {
            return Err(err(
                3,
                format!(
                    "no config at {}; the reference config.j is installed at {}",
                    path,
                    reference_config_path()
                ),
            ));
        }
    };
    match config::load_config(&src) {
        Ok(c) => Ok((c, src)),
        Err(e) => Err(err(3, e.message())),
    }
}

fn reference_config_path() -> String {
    // installed next to the binary under <prefix>/share/j/config.j
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent().and_then(|p| p.parent()) {
            let cand = p.join("share/j/config.j");
            if cand.exists() {
                return cand.display().to_string();
            }
        }
    }
    "/usr/local/share/j/config.j".into()
}

const RESERVED: &[&str] = &["init", "clone", "remote", "fetch", "push", "undo", "redo", "ops"];

fn main() -> ExitCode {
    // evaluation builds deep continuation chains (trampolined, but the final
    // continuation still tears down by recursive Drop), so run the real work
    // on a thread with a large stack to keep very deep histories safe
    std::thread::Builder::new()
        .stack_size(512 * 1024 * 1024)
        .spawn(run)
        .expect("failed to spawn worker thread")
        .join()
        .expect("worker thread panicked")
}

fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // §1: stdin vs arguments
    let mut stdin_content = String::new();
    let stdin_has_data = {
        if std::io::stdin().is_terminal() {
            false
        } else {
            match std::io::stdin().read_to_string(&mut stdin_content) {
                Ok(n) => n > 0,
                Err(_) => false,
            }
        }
    };
    if stdin_has_data && !args.is_empty() {
        return err(2, "expression given both as arguments and on stdin");
    }
    let text = if stdin_has_data {
        stdin_content
    } else if !args.is_empty() {
        args.join(" ")
    } else {
        eprintln!("{}", USAGE);
        return ExitCode::from(2);
    };

    // §1.1: reserved commands
    let trimmed = text.trim();
    let first = trimmed.split_whitespace().next().unwrap_or("");
    if RESERVED.contains(&first) {
        return run_reserved(first, trimmed);
    }

    run_expression(trimmed, true)
}

fn run_reserved(cmd: &str, text: &str) -> ExitCode {
    let words: Vec<&str> = text.split_whitespace().collect();
    match cmd {
        "init" => {
            if words.len() != 1 {
                return err(2, "usage: j init");
            }
            let (cfg, _) = or_exit(load_config_file());
            match j::jj::cmd_init(&cfg) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "clone" => {
            if words.len() != 2 && words.len() != 3 {
                return err(2, "usage: j clone URL [DIR]");
            }
            let (cfg, _) = or_exit(load_config_file());
            let dir = if words.len() == 3 {
                words[2].to_string()
            } else {
                default_clone_dir(words[1])
            };
            match j::jj::cmd_clone(&cfg, words[1], &dir) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "remote" => {
            if words.len() != 2 {
                return err(2, "usage: j remote URL");
            }
            let backend = or_exit(open_repo());
            match backend.cmd_remote(words[1]) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "fetch" => {
            if words.len() != 1 {
                return err(2, "usage: j fetch");
            }
            let (cfg, _) = or_exit(load_config_file());
            let backend = or_exit(open_repo_locked(&cfg));
            match backend.cmd_fetch() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "push" => {
            let rest = text.trim_start_matches("push").trim();
            if rest.is_empty() {
                return err(2, "usage: j push EXPR");
            }
            let (cfg, _) = or_exit(load_config_file());
            let backend = or_exit(open_repo_locked(&cfg));
            // evaluate EXPR against the recorded repository (no snapshot)
            match backend.cmd_push(&cfg, rest) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "undo" => {
            if words.len() != 1 {
                return err(2, "usage: j undo");
            }
            let backend = or_exit(open_repo_noconfig());
            match backend.cmd_undo(false) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "redo" => {
            if words.len() != 1 {
                return err(2, "usage: j redo");
            }
            let backend = or_exit(open_repo_noconfig());
            match backend.cmd_undo(true) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        "ops" => {
            if words.len() != 1 {
                return err(2, "usage: j ops");
            }
            let backend = or_exit(open_repo_noconfig());
            match backend.cmd_ops() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => err(e.0, e.1),
            }
        }
        _ => unreachable!(),
    }
}

fn default_clone_dir(url: &str) -> String {
    let last = url.trim_end_matches('/').rsplit('/').next().unwrap_or("repo");
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

fn or_exit<T>(r: Result<T, ExitCode>) -> T {
    match r {
        Ok(v) => v,
        Err(c) => std::process::exit(exit_code_number(c)),
    }
}

/// ExitCode doesn't expose its value; track it alongside.
fn exit_code_number(c: ExitCode) -> i32 {
    // ExitCode's Debug is "ExitCode(unix_exit_status(N))" on unix
    let s = format!("{:?}", c);
    for tok in s.split(|ch: char| !ch.is_ascii_digit()) {
        if !tok.is_empty() {
            if let Ok(n) = tok.parse() {
                return n;
            }
        }
    }
    1
}

fn open_repo() -> Result<Rc<JjBackend>, ExitCode> {
    match JjBackend::open(None) {
        Ok(b) => Ok(Rc::new(b)),
        Err(e) => Err(err(e.0, e.1)),
    }
}

fn open_repo_noconfig() -> Result<Rc<JjBackend>, ExitCode> {
    // undo/redo/ops do not read the config (§6.1), but take the lock
    match JjBackend::open(None) {
        Ok(b) => {
            b.take_lock();
            Ok(Rc::new(b))
        }
        Err(e) => Err(err(e.0, e.1)),
    }
}

fn open_repo_locked(cfg: &Config) -> Result<Rc<JjBackend>, ExitCode> {
    match JjBackend::open(Some(cfg)) {
        Ok(b) => {
            b.take_lock();
            Ok(Rc::new(b))
        }
        Err(e) => Err(err(e.0, e.1)),
    }
}

fn run_expression(text: &str, snapshot: bool) -> ExitCode {
    // §1.2.1: locate the repository
    let (cfg, _cfg_src) = or_exit(load_config_file());
    let backend = or_exit(open_repo_locked(&cfg));

    // §1.2.2: parse the expression
    let outer = Rc::new(cfg.global_names.clone());
    let mut expr = match parse_expr(text, outer) {
        Ok(e) => e,
        Err(p) => return err(3, format!("line {}: {}", p.line, p.msg)),
    };

    // §1.2.4: build the current repository value (with snapshot)
    let (mut interp, loaded_repo, current_repo) =
        match backend.build_interp(&cfg, text, snapshot) {
            Ok(t) => t,
            Err((2, m)) => return err(2, m),
            Err((_, m)) => return err(1, m),
        };

    // §1.2.5: resolve Id literals in config and expression
    for (def_name, prefix, candidates) in config::config_id_failures(&cfg, &interp) {
        let _ = candidates;
        return err(
            3,
            format!(
                "config.j: `@{}` in `{}` does not resolve to a unique commit",
                prefix, def_name
            ),
        );
    }
    expr = match config::resolve_ids(&expr, &interp) {
        Ok(e) => e,
        Err(failures) => {
            let (prefix, candidates) = &failures[0];
            if candidates.is_empty() {
                eprintln!("j: crash: `@{}` matches no commit", prefix);
            } else {
                eprintln!("j: crash: `@{}` is ambiguous", prefix);
                for c in candidates {
                    eprintln!("   {}", c);
                }
            }
            return ExitCode::from(1);
        }
    };

    // §1.2.6: evaluate config definitions, then the expression
    if let Err(c) = config::eval_config(&mut interp, &cfg) {
        return err(3, format!("config.j: {}", c.msg));
    }
    *interp.old_repo.borrow_mut() = Some(loaded_repo.clone());
    let env = interp.global_env();
    let expr_rc = Rc::new(expr);
    let mut v = match interp.eval(&expr_rc, &env) {
        Ok(v) => v,
        Err(c) => return crash_err(&c, Some(text)),
    };

    // §1.2.7: apply a function result to the repo, once
    if matches!(v, Value::Fun(_)) {
        v = match interp.apply(v, current_repo.clone()) {
            Ok(v) => v,
            Err(c) => return crash_err(&c, Some(text)),
        };
    }

    // §1.2.8: a Repo is persisted (unless unchanged)
    if is_repo_value(&interp, &v) {
        match backend.persist(&mut interp, &cfg, &loaded_repo, &current_repo, &v, text) {
            Ok(()) => ExitCode::SUCCESS,
            Err(c) => crash_err(&c, Some(text)),
        }
    } else {
        // §1.2.9: display
        let color = j::render::color_enabled("auto");
        match j::render::display(&mut interp, &v, color) {
            Ok(s) => {
                print!("{}", s);
                ExitCode::SUCCESS
            }
            Err(c) => crash_err(&c, Some(text)),
        }
    }
}

fn is_repo_value(interp: &Interp, v: &Value) -> bool {
    // §4.4: shape Repo and root's shape Commit
    let fields = match v {
        Value::Record(m) => m.keys().cloned().collect::<BTreeSet<String>>(),
        _ => return false,
    };
    let want: BTreeSet<String> = ["children", "context", "root"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if fields != want {
        return false;
    }
    let root = match v.field("root") {
        Ok(r) => r,
        Err(_) => return false,
    };
    let root_fields = match &root {
        Value::Record(m) => m.keys().cloned().collect::<BTreeSet<String>>(),
        _ => return false,
    };
    let want_root: BTreeSet<String> = ["files", "id", "labels", "message"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let _ = interp;
    root_fields == want_root
}
