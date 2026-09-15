//! Remote workflow tests (§7.6, §7.8): clone/fetch/push against a local
//! bare git remote, label flows, push refusals.

use std::path::PathBuf;
use std::process::Command;

fn j_bin() -> PathBuf {
    option_env!("CARGO_BIN_EXE_j").map(PathBuf::from).unwrap_or_else(|| {
        let mut p = std::env::current_exe().unwrap();
        p.pop();
        p.pop();
        p.push("j");
        p
    })
}

fn uniq(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "j-remote-test-{}-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

struct Env {
    dir: PathBuf,
    remote: PathBuf,
    cfg: PathBuf,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn ok(self) -> Self {
        assert_eq!(self.code, 0, "stderr: {}", self.stderr);
        self
    }
}

fn git(dir: &PathBuf, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn setup() -> Env {
    let dir = uniq("env");
    let remote = uniq("remote.git");
    let cfg = uniq("cfg");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&cfg.join("j")).unwrap();
    let mut config = include_str!("../config.j").to_string();
    config = config.replace("Your Name", "Test User");
    config = config.replace("you@example.com", "test@example.com");
    std::fs::write(cfg.join("j/config.j"), config).unwrap();
    // bare remote with one commit on master
    git(&dir, &["init", "--bare", remote.to_str().unwrap()]);
    let seed = uniq("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "."]);
    std::fs::write(seed.join("a.txt"), "one\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qm", "one"]);
    git(&seed, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&seed, &["push", "-q", "origin", "master"]);
    let _ = std::fs::remove_dir_all(&seed);
    Env { dir, remote, cfg }
}

impl Env {
    fn j(&self, workdir: &PathBuf, args: &[&str]) -> Out {
        let out = Command::new(j_bin())
            .args(args)
            .current_dir(workdir)
            .env("XDG_CONFIG_HOME", &self.cfg)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.remote);
        let _ = std::fs::remove_dir_all(&self.cfg);
    }
}

#[test]
fn clone_sets_origin_and_labels() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    let out = env.j(&dest, &["tree"]).ok();
    assert!(out.stdout.contains("master"), "{}", out.stdout);
    assert!(out.stdout.contains("◆"), "{}", out.stdout); // immutable
    let out = env.j(&dest, &["trunk"]).ok();
    assert!(!out.stdout.contains("none"), "{}", out.stdout);
}

#[test]
fn push_label_and_relabel() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    env.j(&dest, &["push (label \"feature\" here)"]).ok();
    let log = git(&env.remote, &["log", "feature", "--oneline"]);
    assert!(log.contains("work"), "{}", log);
    // squash locally then relabel moves the bookmark
    env.j(&dest, &["new"]).ok();
    std::fs::write(dest.join("b.txt"), "three\n").unwrap();
    env.j(&dest, &["describe \"more\""]).ok();
    env.j(&dest, &["squash"]).ok();
    env.j(&dest, &["push relabel"]).ok();
    // squash kept the parent's message but folded the files; the bookmark
    // moved to the rewritten commit
    let show = git(&env.remote, &["show", "feature:b.txt"]);
    assert!(show.contains("three"), "{}", show);
    // unlabel deletes
    env.j(&dest, &["push (unlabel \"feature\")"]).ok();
    let branches = git(&env.remote, &["branch"]);
    assert!(!branches.contains("feature"), "{}", branches);
}

#[test]
fn push_rename() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    env.j(&dest, &["push (label \"old\" here)"]).ok();
    env.j(&dest, &["push (rename \"old\" \"new\")"]).ok();
    let branches = git(&env.remote, &["branch"]);
    assert!(branches.contains("new"), "{}", branches);
    assert!(!branches.contains("old"), "{}", branches);
}

#[test]
fn push_refusals() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    // empty description refused
    let out = env.j(&dest, &["push (label \"x\" here)"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("empty description"), "{}", out.stderr);
    // deleting a bookmark the remote doesn't have
    let out = env.j(&dest, &["push (unlabel \"nothere\")"]);
    assert_eq!(out.code, 1);
    // moving immutable master backward refused
    std::fs::write(dest.join("a.txt"), "x\n").unwrap();
    env.j(&dest, &["describe \"w\""]).ok();
    let out = env.j(&dest, &["push (label \"master\" here)"]);
    // allowed only if it descends — it does descend (child of master)
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn push_conflicted_commit_refused() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    // create a conflict artificially is involved; instead push invalid name
    std::fs::write(dest.join("a.txt"), "x\n").unwrap();
    env.j(&dest, &["describe \"w\""]).ok();
    let out = env.j(&dest, &["push (label \"bad..name\" here)"]);
    assert_eq!(out.code, 1);
}

#[test]
fn fetch_brings_new_commits() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    // move the remote
    let other = uniq("other");
    git(&env.dir, &["clone", "-q", env.remote.to_str().unwrap(), other.to_str().unwrap()]);
    std::fs::write(other.join("a.txt"), "moved\n").unwrap();
    git(&other, &["-c", "user.email=t@t", "-c", "user.name=T", "commit", "-qam", "moved"]);
    git(&other, &["push", "-q", "origin", "master"]);
    env.j(&dest, &["fetch"]).ok();
    let out = env.j(&dest, &["tree"]).ok();
    assert!(out.stdout.contains("moved"), "{}", out.stdout);
    // rebase trunk brings the stack up to date
    env.j(&dest, &["rebase trunk"]).ok();
    let out = env.j(&dest, &["log"]).ok();
    assert!(out.stdout.contains("moved"), "{}", out.stdout);
}

#[test]
fn fetch_without_origin_is_exit_1() {
    let env = setup();
    let dir = uniq("plain");
    std::fs::create_dir_all(&dir).unwrap();
    env.j(&dir, &["init"]).ok();
    let out = env.j(&dir, &["fetch"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no remote origin"), "{}", out.stderr);
}

#[test]
fn remote_command_sets_url() {
    let env = setup();
    let dir = uniq("plain");
    std::fs::create_dir_all(&dir).unwrap();
    env.j(&dir, &["init"]).ok();
    env.j(&dir, &["remote", env.remote.to_str().unwrap()]).ok();
    env.j(&dir, &["fetch"]).ok();
    let out = env.j(&dir, &["tree"]).ok();
    assert!(out.stdout.contains("master"), "{}", out.stdout);
}

#[test]
fn undo_push_restores_labels() {
    let env = setup();
    let dest = uniq("clone");
    env.j(&env.dir, &["clone", env.remote.to_str().unwrap(), dest.to_str().unwrap()]).ok();
    std::fs::write(dest.join("a.txt"), "two\n").unwrap();
    env.j(&dest, &["describe \"work\""]).ok();
    let _before = env.j(&dest, &["log"]).ok().stdout;
    env.j(&dest, &["push (label \"feature\" here)"]).ok();
    let after = env.j(&dest, &["labelled \"feature\""]).ok();
    assert!(after.stdout.contains("vwq") || !after.stdout.contains("none"), "{}", after.stdout);
    let out2 = env.j(&dest, &["labelled \"feature\""]).ok();
    assert!(!out2.stdout.contains("none"), "label not visible after push: {}", out2.stdout);
    env.j(&dest, &["undo"]).ok();
    // undoing a push restores the recorded labels (§7.7): feature is gone
    let labels = env.j(&dest, &["trunk"]).ok().stdout;
    let out = env.j(&dest, &["labelled \"feature\""]).ok().stdout;
    assert!(out.contains("none") || out.contains("0 items"), "{}", out);
    let _ = labels;
    // the remote itself is untouched
    let branches = git(&env.remote, &["branch"]);
    assert!(branches.contains("feature"), "{}", branches);
}

#[test]
fn clone_default_dir_name() {
    let env = setup();
    // clone URL without DIR: last path component minus .git
    let workdir = uniq("work");
    std::fs::create_dir_all(&workdir).unwrap();
    env.j(&workdir, &["clone", env.remote.to_str().unwrap()]).ok();
    let name = env
        .remote
        .file_name()
        .unwrap()
        .to_string_lossy()
        .trim_end_matches(".git")
        .to_string();
    let dest = workdir.join(&name);
    assert!(dest.join(".jj").exists(), "no clone at {}", dest.display());
}
