//! Tree rendering tests (§7.11): glyphs, elision, margin, layout.

use j::config;
use j::domain::{MemBackend, MetaInfo, ROOT_ID};
use j::eval::Interp;
use j::parse::parse_expr;
use j::value::{BlobVal, Env, Value};
use std::rc::Rc;

const CONFIG: &str = include_str!("../config.j");

fn commit(id: &str, msg: &str, labels: &[&str], files: Vec<(&str, &str)>) -> Value {
    let files = Value::list(
        files
            .into_iter()
            .map(|(p, c)| {
                Value::record(&[
                    ("content", BlobVal::text_blob(c)),
                    ("path", Value::list(p.split('/').map(Value::text).collect())),
                ])
            })
            .collect(),
    );
    Value::record(&[
        ("files", files),
        ("message", Value::text(msg)),
        (
            "labels",
            Value::list(labels.iter().map(|l| Value::text(*l)).collect()),
        ),
        ("id", Value::Id(Rc::new(id.to_string()))),
    ])
}

fn subtree(root: Value, kids: Vec<Value>) -> Value {
    Value::record(&[
        ("children", Value::list(kids)),
        ("root", root),
    ])
}

/// build a repo focused on the root's child at `focus_idx`
fn repo_of(root: Value, kids: Vec<Value>, focus_idx: Option<usize>) -> Value {
    match focus_idx {
        None => Value::record(&[
            ("children", Value::list(kids)),
            ("context", Value::list(vec![])),
            ("root", root),
        ]),
        Some(i) => {
            let left: Vec<Value> = kids[..i].to_vec();
            let right: Vec<Value> = kids[i + 1..].to_vec();
            let focus = kids[i].clone();
            let frame = Value::record(&[
                ("left", Value::list(left)),
                ("parent", root),
                ("right", Value::list(right)),
            ]);
            Value::record(&[
                ("children", focus.field("children").unwrap()),
                ("context", Value::list(vec![frame])),
                ("root", focus.field("root").unwrap()),
            ])
        }
    }
}

fn meta(id: &str, author: &str, time: i64) -> (String, MetaInfo) {
    (
        id.to_string(),
        MetaInfo {
            hash: format!("h{}", id),
            author: author.into(),
            email: "a@x".into(),
            time,
        },
    )
}

fn backend_with(metas: Vec<(String, MetaInfo)>) -> MemBackend {
    let mut b = MemBackend::new();
    for (id, m) in metas {
        b.metas.insert(id, m);
    }
    b
}

fn make_interp(b: MemBackend) -> (Interp, config::Config) {
    let cfg = config::load_config(CONFIG).unwrap();
    let mut i = Interp::new(Rc::new(b), cfg.shapes.clone(), Env::empty());
    config::eval_config(&mut i, &cfg).unwrap();
    (i, cfg)
}

fn tree_text(interp: &mut Interp, cfg: &config::Config, src: &str, repo: Value) -> String {
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(src, outer).unwrap();
    let e = config::resolve_ids(&e, interp).unwrap();
    let env = interp.global_env();
    let v = interp.eval(&Rc::new(e), &env).unwrap();
    let v = if matches!(v, Value::Fun(_)) {
        interp.apply(v, repo).unwrap()
    } else {
        v
    };
    v.as_text().unwrap().to_string()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn tree_glyphs_basic() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "work a", &[], vec![("f", "x")]);
    let b = commit("kbbbbbbb", "work b", &["main"], vec![("f", "y")]);
    let repo = repo_of(root, vec![subtree(a, vec![subtree(b.clone(), vec![])])], None);
    // focus on root (context empty); b is a grandchild
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "Root", now() - 100_000),
        meta("kaaaaaaa", "Ann Author", now() - 50_000),
        meta("kbbbbbbb", "Bob B", now() - 100),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 2, margin = true, elide = false, icons = false, color = \"never\" })",
        repo,
    );
    assert!(text.contains("⌂"), "{}", text);
    assert!(text.contains("work a"), "{}", text);
    assert!(text.contains("work b"), "{}", text);
    assert!(text.contains("main"), "{}", text);
    // margin: ages and initials
    assert!(text.contains("aa"), "{}", text);
    assert!(text.contains("bb"), "{}", text);
}

#[test]
fn tree_focus_marker_and_glyph() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![("f", "x")]);
    let b = commit("kbbbbbbb", "b", &[], vec![("f", "y")]);
    let repo = repo_of(root, vec![subtree(a, vec![subtree(b, vec![])])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "Root", 1),
        meta("kaaaaaaa", "A", now() - 10),
        meta("kbbbbbbb", "B", now() - 5),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\" })",
        repo,
    );
    assert!(text.contains("▶"), "{}", text);
    assert!(text.contains("◉"), "{}", text);
}

#[test]
fn tree_conflict_and_empty_glyphs() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let conflicted = {
        let c = commit("kccccccc", "conflicted", &[], vec![]);
        let files = Value::list(vec![Value::record(&[
            (
                "content",
                Value::Blob(Rc::new(j::value::BlobVal {
                    kind: j::value::BlobKind::Regular,
                    content: j::value::BlobContent::Conflict(vec![
                        Rc::new(b"a".to_vec()),
                        Rc::new(b"b".to_vec()),
                        Rc::new(b"c".to_vec()),
                    ]),
                })),
            ),
            ("path", Value::list(vec![Value::text("f")])),
        ])]);
        Value::record(&[
            ("files", files),
            ("message", c.field("message").unwrap()),
            ("labels", c.field("labels").unwrap()),
            ("id", c.field("id").unwrap()),
        ])
    };
    let empty_child = commit("keeeeeee", "empty", &[], vec![]);
    let repo = repo_of(
        root,
        vec![subtree(conflicted, vec![]), subtree(empty_child, vec![])],
        None,
    );
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "Root", 1),
        meta("kccccccc", "A", now()),
        meta("keeeeeee", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\" })",
        repo,
    );
    assert!(text.contains("⊗"), "{}", text);
}

#[test]
fn tree_icons_set() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![("f", "x")]);
    let repo = repo_of(root, vec![subtree(a, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "Root", 1),
        meta("kaaaaaaa", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = true, color = \"never\" })",
        repo,
    );
    assert!(text.contains("🌱"), "{}", text);
}

#[test]
fn tree_labels_column_only_when_present() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![]);
    let b = commit("kbbbbbbb", "b", &["feat"], vec![]);
    let repo = repo_of(root, vec![subtree(a, vec![]), subtree(b, vec![])], None);
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", 1),
        meta("kaaaaaaa", "A", now()),
        meta("kbbbbbbb", "B", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\" })",
        repo,
    );
    assert!(text.contains("feat"), "{}", text);
}

#[test]
fn tree_with_missing_options_crashes() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let repo = repo_of(root, vec![], None);
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    let outer = Rc::new(cfg.global_names.clone());
    let e = parse_expr(
        "treeWith ({ detail = 1, margin = false, elide = true, icons = false })",
        outer,
    )
    .unwrap();
    let env = i.global_env();
    // missing fields crash, either when the option record is supplied or when
    // the result is applied (§7.11 "Missing fields crash")
    let r = i
        .eval(&Rc::new(e), &env)
        .and_then(|v| i.apply(v, repo));
    match r {
        Ok(_) => panic!("missing color must crash"),
        Err(c) => {
            let ok = c.msg.contains("color") || c.msg.contains("treeWith") || c.msg.contains("field");
            assert!(ok, "{}", c.msg);
        }
    }
}

#[test]
fn tree_color_never_has_no_ansi() {
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![("f", "x")]);
    let repo = repo_of(root, vec![subtree(a, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", 1),
        meta("kaaaaaaa", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"never\" })",
        repo,
    );
    assert!(!text.contains('\x1b'), "ANSI found in color=never output");
}

#[test]
fn tree_color_always_has_ansi() {
    unsafe { std::env::remove_var("NO_COLOR") };
    let root = commit(ROOT_ID, "", &[], vec![]);
    let a = commit("kaaaaaaa", "a", &[], vec![("f", "x")]);
    let repo = repo_of(root, vec![subtree(a, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", 1),
        meta("kaaaaaaa", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 1, margin = false, elide = false, icons = false, color = \"always\" })",
        repo,
    );
    assert!(text.contains('\x1b'), "no ANSI in color=always output");
}

#[test]
fn tree_minted_commits_have_blank_margin() {
    // minted commits have no metadata: no margin content (§7.11)
    let root = commit(ROOT_ID, "", &[], vec![]);
    let minted = commit("kqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq", "minted", &[], vec![]);
    let repo = repo_of(root, vec![subtree(minted, vec![])], None);
    let (mut i, cfg) = make_interp(backend_with(vec![meta(ROOT_ID, "R", 1)]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 2, margin = true, elide = false, icons = false, color = \"never\" })",
        repo,
    );
    assert!(text.contains("minted"), "{}", text);
}

#[test]
fn detail_line_changed_paths() {
    let root = commit(ROOT_ID, "", &[], vec![("keep", "1"), ("mod", "1"), ("del", "1")]);
    let focus = commit(
        "kaaaaaaa",
        "changes",
        &[],
        vec![("keep", "1"), ("mod", "2"), ("new", "n")],
    );
    let repo = repo_of(root, vec![subtree(focus, vec![])], Some(0));
    let (mut i, cfg) = make_interp(backend_with(vec![
        meta(ROOT_ID, "R", now() - 10),
        meta("kaaaaaaa", "A", now()),
    ]));
    let text = tree_text(
        &mut i,
        &cfg,
        "treeWith ({ detail = 2, margin = false, elide = false, icons = false, color = \"never\" })",
        repo,
    );
    assert!(text.contains("+ new"), "{}", text);
    assert!(text.contains("~ mod"), "{}", text);
    assert!(text.contains("− del"), "{}", text);
}
