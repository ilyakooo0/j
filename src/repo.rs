//! Repo-zipper operations, the reference `by` walk (§10), and the
//! persistence validation shared by `validate` and the jj backend (§7.5).

use crate::domain::ROOT_ID;
use crate::eval::Interp;
use crate::value::{value_eq, Crash, Value};
use std::collections::{BTreeMap, BTreeSet};

/// navigate: refocus `repo` on the commit with change id `id` (§10 reference).
pub fn by_id(repo: &Value, id: &str) -> Result<Option<Value>, Crash> {
    let top = top_of(repo)?;
    let mut stack = vec![top];
    while let Some(loc) = stack.pop() {
        let root_id = id_of(&loc.field("root")?)?;
        if root_id == id {
            return Ok(Some(loc));
        }
        let children = loc.field("children").and_then(|v| v.as_list().map(|x| x.to_vec()))?;
        // push children as refocused locations; iterate in order
        for child in children.iter() {
            stack.push(refocus_child(&loc, child)?);
        }
    }
    Ok(None)
}

fn id_of(commit: &Value) -> Result<String, Crash> {
    match commit.field("id")? {
        Value::Id(i) => Ok(i.to_string()),
        v => Err(Crash::new(format!("expected an Id, got a {}", v.kind_name()))),
    }
}

/// move a location to the top of history
fn top_of(repo: &Value) -> Result<Value, Crash> {
    let mut cur = repo.clone();
    loop {
        let ctx = cur.field("context").and_then(|v| v.as_list().map(|x| x.to_vec()))?;
        if ctx.is_empty() {
            return Ok(cur);
        }
        cur = up_of(&cur)?;
    }
}

fn up_of(repo: &Value) -> Result<Value, Crash> {
    let ctx = repo.field("context")?;
    let ctx = ctx.as_list()?;
    let frame = ctx
        .first()
        .ok_or_else(|| Crash::new("up: at the top of history"))?;
    let parent = frame.field("parent")?;
    let left = frame.field("left")?;
    let left = left.as_list()?;
    let right = frame.field("right")?;
    let right = right.as_list()?;
    let mut children: Vec<Value> = left.to_vec();
    children.push(Value::record(&[
        ("root", repo.field("root")?),
        ("children", repo.field("children")?),
    ]));
    children.extend_from_slice(right);
    Ok(Value::record(&[
        ("children", Value::list(children)),
        ("context", Value::list(ctx[1..].to_vec())),
        ("root", parent),
    ]))
}

fn refocus_child(repo: &Value, child: &Value) -> Result<Value, Crash> {
    let children = repo.field("children")?;
    let children = children.as_list()?;
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut found = false;
    // match by the child's root id: ids are unique per repo (§7.5 validates
    // this at persist time), and a deep value_eq here would compare the whole
    // subtree — files included — making by_id quadratic in history size
    let want_id = id_of(&child.field("root")?).ok();
    for c in children.iter() {
        let same = match (&want_id, id_of(&c.field("root")?).ok()) {
            (Some(w), Some(cid)) => *w == cid,
            _ => value_eq(c, child)?,
        };
        if !found && same {
            found = true;
        } else if !found {
            left.push(c.clone());
        } else {
            right.push(c.clone());
        }
    }
    let frame = Value::record(&[
        ("left", Value::list(left)),
        ("parent", repo.field("root")?),
        ("right", Value::list(right)),
    ]);
    let mut ctx = repo.field("context")?.as_list()?.to_vec();
    ctx.insert(0, frame);
    Ok(Value::record(&[
        ("children", child.field("children")?),
        ("context", Value::list(ctx)),
        ("root", child.field("root")?),
    ]))
}

// ----------------------------------------------------------------------
// tree traversal helpers over Repo values
// ----------------------------------------------------------------------

/// all commits in the whole history (preorder from the top)
pub fn all_commits(repo: &Value) -> Result<Vec<Value>, Crash> {
    let top = top_of(repo)?;
    let mut out = Vec::new();
    let children = top.field("children")?;
    collect_commits(&top.field("root")?, children.as_list()?, &mut out)?;
    Ok(out)
}

fn collect_commits(root: &Value, children: &[Value], out: &mut Vec<Value>) -> Result<(), Crash> {
    out.push(root.clone());
    for c in children {
        let gc = c.field("children")?;
        collect_commits(&c.field("root")?, gc.as_list()?, out)?;
    }
    Ok(())
}

/// parent change id of every commit (first-parent tree), as (child_id, parent_id or "")
pub fn parent_map(repo: &Value) -> Result<Vec<(String, String)>, Crash> {
    let top = top_of(repo)?;
    let mut out = Vec::new();
    parent_map_rec(&top, &mut out)?;
    Ok(out)
}

fn parent_map_rec(loc: &Value, out: &mut Vec<(String, String)>) -> Result<(), Crash> {
    let pid = id_of(&loc.field("root")?)?;
    let children = loc.field("children")?;
    for c in children.as_list()?.iter() {
        out.push((id_of(&c.field("root")?)?, pid.clone()));
        parent_map_rec(&refocus_child(loc, c)?, out)?;
    }
    Ok(())
}

// ----------------------------------------------------------------------
// validation (§7.5 steps 1–3 and 6), shared by `validate` and persistence
// ----------------------------------------------------------------------

#[derive(Debug)]
pub struct Validated {
    pub immutable: BTreeSet<String>,
}

pub fn validate_repo(i: &mut Interp, new: &Value) -> Result<Validated, Crash> {
    // shape: new must be a Repo with a Commit root (§4.4)
    check_repo_shape(new)?;
    let mut seen = BTreeSet::new();
    validate_ids_and_snapshots(new, &mut seen)?;
    // immutability needs `old`; the builtin validate uses the repo loaded at
    // startup, carried in the interpreter by the CLI
    let old = i.old_repo.borrow().clone();
    let immutable = match &old {
        Some(old) => {
            validate_labels(old, new)?;
            compute_immutable(i, old)?
        }
        None => BTreeSet::new(),
    };
    if let Some(old) = &old {
        validate_immutable(old, new, &immutable)?;
    }
    // focus mutable (§7.5 step 6)
    let focus_id = id_of(&new.field("root")?)?;
    if focus_id == ROOT_ID || immutable.contains(&focus_id) {
        return Err(Crash::new(
            "the focus must be a mutable commit; compose with new",
        ));
    }
    Ok(Validated { immutable })
}

fn check_repo_shape(v: &Value) -> Result<(), Crash> {
    let fields = v
        .field_set()
        .ok_or_else(|| Crash::new("persistence: the value is not a Repo"))?;
    let want: BTreeSet<String> = ["children", "context", "root"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if fields != want {
        return Err(Crash::new("persistence: the value is not a Repo"));
    }
    let root_fields = v
        .field("root")?
        .field_set()
        .ok_or_else(|| Crash::new("persistence: the root is not a Commit"))?;
    let want_root: BTreeSet<String> = ["files", "id", "labels", "message"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if root_fields != want_root {
        return Err(Crash::new("persistence: the root is not a Commit"));
    }
    Ok(())
}

fn validate_ids_and_snapshots(repo: &Value, seen: &mut BTreeSet<String>) -> Result<(), Crash> {
    let top = top_of(repo)?;
    validate_tree(&top, seen)
}

fn validate_tree(loc: &Value, seen: &mut BTreeSet<String>) -> Result<(), Crash> {
    let root = loc.field("root")?;
    let id = id_of(&root)?;
    if !seen.insert(id.clone()) {
        return Err(Crash::new(format!(
            "persistence: id {} occurs more than once",
            id
        )));
    }
    let files = root.field("files")?;
    let files = files.as_list()?;
    let mut paths = BTreeSet::new();
    for e in files {
        let p = e.field("path")?;
        let p = p.as_list()?;
        let key: Vec<String> = p
            .iter()
            .map(|c| c.as_text().map(|s| s.to_string()))
            .collect::<Result<_, _>>()?;
        if !paths.insert(key) {
            return Err(Crash::new("persistence: a snapshot has duplicate paths"));
        }
    }
    let children = loc.field("children")?;
    for c in children.as_list()?.iter() {
        validate_tree(&refocus_child(loc, c)?, seen)?;
    }
    Ok(())
}

fn label_set(repo: &Value) -> Result<BTreeSet<(String, String)>, Crash> {
    let mut out = BTreeSet::new();
    for c in all_commits(repo)? {
        let id = id_of(&c)?;
        for l in c.field("labels").and_then(|v| v.as_list().map(|x| x.to_vec()))?.iter() {
            out.insert((id.clone(), l.as_text()?.to_string()));
        }
    }
    Ok(out)
}

fn validate_labels(old: &Value, new: &Value) -> Result<(), Crash> {
    let a = label_set(old)?;
    let b = label_set(new)?;
    if a != b {
        return Err(Crash::new(
            "persistence: labels are read-only; the set of (id, label) pairs changed",
        ));
    }
    Ok(())
}

/// The immutable set: the config's `immutable` revset against `old`, plus
/// every merge commit and its ancestors (§7.5 step 3).
pub fn compute_immutable(i: &mut Interp, old: &Value) -> Result<BTreeSet<String>, Crash> {
    let v = i.apply_cached_revset("immutable", old)?;
    let mut set = BTreeSet::new();
    for idv in v.as_list()?.iter() {
        match idv {
            Value::Id(id) => {
                set.insert(id.to_string());
            }
            _ => return Err(Crash::new("immutable: revset returned a non-Id")),
        }
    }
    // merge commits and their ancestors
    let mut merges = BTreeSet::new();
    for c in all_commits(old)? {
        let id = id_of(&c)?;
        if i.backend.is_merge(&id) {
            merges.insert(id);
        }
    }
    let closed = i.backend.ancestors_closed(&merges);
    set.extend(closed);
    Ok(set)
}

fn validate_immutable(old: &Value, new: &Value, immutable: &BTreeSet<String>) -> Result<(), Crash> {
    if immutable.is_empty() {
        return Ok(());
    }
    let old_parents: BTreeMap<String, String> = parent_map(old)?.into_iter().collect();
    let new_parents: BTreeMap<String, String> = parent_map(new)?.into_iter().collect();
    let new_commits: BTreeMap<String, Value> = all_commits(new)?
        .into_iter()
        .map(|c| id_of(&c).map(|id| (id, c)))
        .collect::<Result<_, _>>()?;
    for c in all_commits(old)? {
        let id = id_of(&c)?;
        if !immutable.contains(&id) {
            continue;
        }
        let nc = new_commits.get(&id).ok_or_else(|| {
            Crash::new(format!("persistence: commit {} is immutable", id))
        })?;
        let same_parent = old_parents.get(&id).cloned().unwrap_or_default()
            == new_parents.get(&id).cloned().unwrap_or_default();
        if !same_parent {
            return Err(Crash::new(format!(
                "persistence: commit {} is immutable (parent changed)",
                id
            )));
        }
        if !value_eq(&c.field("files")?, &nc.field("files")?)? {
            return Err(Crash::new(format!(
                "persistence: commit {} is immutable (files changed)",
                id
            )));
        }
        if !value_eq(&c.field("message")?, &nc.field("message")?)? {
            return Err(Crash::new(format!(
                "persistence: commit {} is immutable (message changed)",
                id
            )));
        }
    }
    Ok(())
}
