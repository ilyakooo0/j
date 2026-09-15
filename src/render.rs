//! Display (§5.1) and tree rendering (§7.11).

use crate::domain::ROOT_ID;
use crate::eval::Interp;
use crate::value::{Crash, ShapeKind, Value};
use std::collections::{BTreeMap, BTreeSet};
use unicode_width::UnicodeWidthChar;

// ----------------------------------------------------------------------
// colour
// ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub struct Palette {
    pub on: bool,
}

impl Palette {
    fn wrap(&self, code: &str, s: &str) -> String {
        if self.on {
            format!("\x1b[{}m{}\x1b[0m", code, s)
        } else {
            s.to_string()
        }
    }
    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
    pub fn bold_bg(&self, s: &str) -> String {
        self.wrap("1;48;5;236", s)
    }
    pub fn dim_italic(&self, s: &str) -> String {
        self.wrap("2;3", s)
    }
    pub fn cyan(&self, s: &str) -> String {
        self.wrap("36", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
    pub fn blue(&self, s: &str) -> String {
        self.wrap("34", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn grey(&self, bright: u8, s: &str) -> String {
        // grey scale: brighter when more recent
        self.wrap(&format!("38;5;{}", 245 + bright.min(10)), s)
    }
    pub fn accent(&self, level: u8, s: &str) -> String {
        // size bar: one accent colour, brighter with size
        let codes = ["38;5;240", "38;5;36", "38;5;32", "38;5;39", "38;5;87"];
        self.wrap(codes[level.min(4) as usize], s)
    }
    pub fn author(&self, name: &str, s: &str) -> String {
        // stable hue per author
        let mut h: u32 = 5381;
        for b in name.bytes() {
            h = h.wrapping_mul(33).wrapping_add(b as u32);
        }
        let hue = (h % 6) as u8;
        let code = ["31", "32", "33", "34", "35", "36"][hue as usize];
        self.wrap(code, s)
    }
}

pub fn color_enabled(mode: &str) -> bool {
    if std::env::var_os("NO_COLOR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    match mode {
        "always" => true,
        "never" => false,
        _ => crate::show::stdout_is_tty(),
    }
}

// display width with East Asian wide = 2, combining = 0
pub fn width(s: &str) -> usize {
    let mut w = 0;
    let mut esc = false;
    for c in s.chars() {
        if esc {
            if c == 'm' {
                esc = false;
            }
            continue;
        }
        if c == '\x1b' {
            esc = true;
            continue;
        }
        w += UnicodeWidthChar::width(c).unwrap_or(0);
    }
    w
}

fn pad_right(s: &str, w: usize) -> String {
    let d = width(s);
    if d >= w {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(w - d))
    }
}

fn human_size(n: usize) -> String {
    if n < 1024 {
        format!("{} B", n)
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}

// ----------------------------------------------------------------------
// shape recognition (§4.12/§5.1)
// ----------------------------------------------------------------------

fn field_set(v: &Value) -> Option<BTreeSet<String>> {
    match v {
        Value::Record(m) => Some(m.keys().cloned().collect()),
        _ => None,
    }
}

fn shape_of_record(interp: &Interp, v: &Value) -> Option<String> {
    let fs = field_set(v)?;
    for (name, _) in &interp.shapes.decls {
        if let Some(s) = interp.shapes.shape_of(name) {
            if let ShapeKind::Record(want) = &s.kind {
                if want == &fs {
                    return Some(name.clone());
                }
            }
        }
    }
    None
}

pub fn is_commit(interp: &Interp, v: &Value) -> bool {
    shape_of_record(interp, v).as_deref() == Some("Commit")
}

// ----------------------------------------------------------------------
// display
// ----------------------------------------------------------------------

pub fn display(interp: &mut Interp, v: &Value, color: bool) -> Result<String, Crash> {
    let pal = Palette { on: color };
    let mut out = String::new();
    display_block(interp, v, &pal, 0, &mut out)?;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn display_id(interp: &Interp, id: &str, pal: &Palette) -> String {
    let p = interp.backend.unique_prefix(id);
    let n = p.len();
    format!("{}{}", pal.bold(p.as_str()), pal.dim(&id[n..]))
}

fn display_line(interp: &Interp, v: &Value, pal: &Palette) -> Result<String, Crash> {
    match v {
        Value::Int(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Text(t) => {
            let first = t.lines().next().unwrap_or("");
            if t.lines().count() > 1 {
                Ok(format!("{}…", first))
            } else {
                Ok(first.to_string())
            }
        }
        Value::Id(id) => Ok(display_id(interp, id, pal)),
        Value::Blob(b) => {
            let size = human_size(b.size());
            if b.is_unresolved() {
                Ok(format!("‹✖ {}›", size))
            } else {
                Ok(format!("‹{}›", size))
            }
        }
        Value::Fun(f) => Ok(crate::show::show(&Interp::dummy(), &Value::Fun(f.clone()))),
        Value::Shape(s) => Ok(s.name.clone()),
        Value::Record(_) => {
            if let Some(shape) = shape_of_record(interp, v) {
                match shape.as_str() {
                    "Commit" => return commit_line(interp, v, pal, false, &BTreeSet::new(), false),
                    "Entry" => return entry_line(interp, v, pal),
                    "Subtree" => {
                        return commit_line(
                            interp,
                            &v.field("root")?,
                            pal,
                            false,
                            &BTreeSet::new(),
                            false,
                        )
                    }
                    "Repo" => {
                        return commit_line(
                            interp,
                            &v.field("root")?,
                            pal,
                            true,
                            &BTreeSet::new(),
                            false,
                        )
                    }
                    "Frame" => {
                        return commit_line(
                            interp,
                            &v.field("parent")?,
                            pal,
                            false,
                            &BTreeSet::new(),
                            false,
                        )
                    }
                    "Change" => {
                        let n = touched_paths(v)?.len();
                        return Ok(format!("{} paths", n));
                    }
                    _ => {}
                }
            }
            let names: Vec<String> = field_set(v).unwrap().into_iter().collect();
            Ok(format!("{{ {} }}", names.join(", ")))
        }
        Value::List(xs) => list_line(interp, xs, pal),
    }
}

fn list_line(_interp: &Interp, xs: &[Value], pal: &Palette) -> Result<String, Crash> {
    if xs.is_empty() {
        return Ok("0 items".into());
    }
    let all_id = xs.iter().all(|x| matches!(x, Value::Id(_)));
    if all_id {
        return Ok(format!("{} commits", xs.len()));
    }
    let all_path = xs.iter().all(|x| match x {
        Value::List(p) => p.iter().all(|c| matches!(c, Value::Text(_))),
        _ => false,
    });
    if all_path {
        return Ok(format!("{} paths", xs.len()));
    }
    let all_text = xs.iter().all(|x| matches!(x, Value::Text(_)));
    if all_text {
        return Ok(format!("{} items", xs.len()));
    }
    let all_record = xs.iter().all(|x| matches!(x, Value::Record(_)));
    if all_record {
        let first = field_set(&xs[0]).unwrap();
        let same = xs
            .iter()
            .all(|x| field_set(x).map(|s| s == first).unwrap_or(false));
        if same {
            return Ok(format!("{} rows", xs.len()));
        }
    }
    let all_scalar = xs.iter().all(|x| {
        matches!(x, Value::Int(_) | Value::Bool(_))
    });
    if all_scalar {
        let parts: Vec<String> = xs
            .iter()
            .map(|x| match x {
                Value::Int(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => unreachable!(),
            })
            .collect();
        return Ok(parts.join(", "));
    }
    let _ = pal;
    Ok(format!("{} items", xs.len()))
}

fn entry_line(interp: &Interp, v: &Value, pal: &Palette) -> Result<String, Crash> {
    let path = path_string(&v.field("path")?)?;
    let content = v.field("content")?;
    let (size, unresolved) = match &content {
        Value::Blob(b) => (human_size(b.size()), b.is_unresolved()),
        _ => (String::new(), false),
    };
    let _ = interp;
    if unresolved {
        Ok(format!("{} {}  {}", pal.red("✖"), path, size))
    } else {
        Ok(format!("  {}  {}", path, size))
    }
}

fn path_string(p: &Value) -> Result<String, Crash> {
    let comps = p.as_list()?;
    let parts: Vec<String> = comps
        .iter()
        .map(|c| c.as_text().map(|s| s.to_string()))
        .collect::<Result<_, _>>()?;
    Ok(parts.join("/"))
}

fn touched_paths(change: &Value) -> Result<Vec<(String, char, bool)>, Crash> {
    // (path, mark, unresolved)
    let from = snapshot_map_of(&change.field("from")?)?;
    let to = snapshot_map_of(&change.field("to")?)?;
    let mut paths: BTreeSet<Vec<String>> = BTreeSet::new();
    paths.extend(from.keys().cloned());
    paths.extend(to.keys().cloned());
    let mut out = Vec::new();
    for p in paths {
        let f = from.get(&p);
        let t = to.get(&p);
        let mark = match (f, t) {
            (None, Some(_)) => '+',
            (Some(_), None) => '−',
            (Some(a), Some(b)) => {
                if crate::value::value_eq(a, b)? {
                    continue;
                }
                '~'
            }
            (None, None) => continue,
        };
        let unresolved = t
            .map(|b| matches!(b, Value::Blob(bl) if bl.is_unresolved()))
            .unwrap_or(false);
        out.push((p.join("/"), if unresolved { '✖' } else { mark }, unresolved));
    }
    Ok(out)
}

fn snapshot_map_of(snap: &Value) -> Result<BTreeMap<Vec<String>, Value>, Crash> {
    let mut m = BTreeMap::new();
    for e in snap.as_list()?.iter() {
        let path = e
            .field("path")?
            .as_list()?
            .iter()
            .map(|c| c.as_text().map(|s| s.to_string()))
            .collect::<Result<Vec<String>, Crash>>()?;
        m.insert(path, e.field("content")?);
    }
    Ok(m)
}

fn commit_line(
    interp: &Interp,
    c: &Value,
    pal: &Palette,
    focus: bool,
    immutable: &BTreeSet<String>,
    has_conflicts_hint: bool,
) -> Result<String, Crash> {
    let id = match c.field("id")? {
        Value::Id(i) => i.to_string(),
        _ => return Err(Crash::new("commit without id")),
    };
    let msg = c.field("message")?.as_text()?.to_string();
    let msg_first = msg.lines().next().unwrap_or("").to_string();
    let labels: Vec<String> = c
        .field("labels")?
        .as_list()?
        .iter()
        .map(|l| l.as_text().map(|s| s.to_string()))
        .collect::<Result<_, _>>()?;
    let glyph = node_glyph(interp, c, &id, focus, immutable, pal)?;
    let id_s = display_id(interp, &id, pal);
    let labels_s = if labels.is_empty() {
        String::new()
    } else {
        pal.green(&labels.join("  "))
    };
    let _ = has_conflicts_hint;
    Ok(if labels_s.is_empty() {
        format!("{} {}  {}", glyph, id_s, msg_first)
    } else {
        format!("{} {}  {}  {}", glyph, id_s, msg_first, labels_s)
    })
}

fn node_glyph(
    interp: &Interp,
    c: &Value,
    id: &str,
    focus: bool,
    immutable: &BTreeSet<String>,
    pal: &Palette,
) -> Result<String, Crash> {
    let g = if id == ROOT_ID {
        "⌂"
    } else if has_conflict(c)? {
        "⊗"
    } else if is_empty_commit(interp, c)? {
        "◌"
    } else if immutable.contains(id) {
        "◆"
    } else if focus {
        "◉"
    } else {
        "○"
    };
    let _ = pal;
    Ok(g.to_string())
}

fn has_conflict(c: &Value) -> Result<bool, Crash> {
    for e in c.field("files").and_then(|v| v.as_list().map(|x| x.to_vec()))?.iter() {
        if let Value::Blob(b) = e.field("content")? {
            if b.is_unresolved() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// empty: no change against its parent — needs the parent; we only compute
/// this meaningfully in tree rendering. In line display, treat as false.
fn is_empty_commit(_interp: &Interp, _c: &Value) -> Result<bool, Crash> {
    Ok(false)
}

fn display_block(
    interp: &mut Interp,
    v: &Value,
    pal: &Palette,
    indent: usize,
    out: &mut String,
) -> Result<(), Crash> {
    let pad = " ".repeat(indent);
    match v {
        Value::Text(t) => {
            out.push_str(t);
            return Ok(());
        }
        Value::Blob(b) => {
            let content = String::from_utf8_lossy(&b.bytes()).to_string();
            out.push_str(&indent_multiline(&content, indent));
            return Ok(());
        }
        Value::Int(_) | Value::Bool(_) | Value::Id(_) | Value::Fun(_) | Value::Shape(_) => {
            out.push_str(&pad);
            out.push_str(&display_line(interp, v, pal)?);
            out.push('\n');
            return Ok(());
        }
        Value::List(xs) => return display_list_block(interp, xs, pal, indent, out),
        Value::Record(_) => {}
    }
    if let Some(shape) = shape_of_record(interp, v) {
        match shape.as_str() {
            "Commit" => return display_commit_block(interp, v, pal, indent, out),
            "Entry" => {
                out.push_str(&pad);
                out.push_str(&entry_line(interp, v, pal)?);
                out.push('\n');
                return Ok(());
            }
            "Subtree" => {
                let opts = default_tree_options(interp)?;
                let repo = Value::record(&[
                    ("root", v.field("root")?),
                    ("children", v.field("children")?),
                    ("context", Value::list(vec![])),
                ]);
                let text = tree_render(interp, &opts, &repo, pal, false)?;
                out.push_str(&indent_multiline(&text, indent));
                return Ok(());
            }
            "Repo" => {
                let tree_fn = interp
                    .globals
                    .lookup("tree")
                    .ok_or_else(|| Crash::new("`tree` is not defined"))?;
                let text = interp.apply(tree_fn, v.clone())?;
                let text = text.as_text()?;
                out.push_str(&indent_multiline(text, indent));
                return Ok(());
            }
            "Frame" => {
                out.push_str(&pad);
                out.push_str(&commit_line(
                    interp,
                    &v.field("parent")?,
                    pal,
                    false,
                    &BTreeSet::new(),
                    false,
                )?);
                out.push('\n');
                return Ok(());
            }
            "Change" => {
                for (p, mark, unresolved) in touched_paths(v)? {
                    out.push_str(&pad);
                    let ms = mark.to_string();
                    let colored = match mark {
                        '+' => pal.green(&ms),
                        '−' => pal.red(&ms),
                        '~' => pal.yellow(&ms),
                        '✖' => pal.red(&ms),
                        _ => ms,
                    };
                    let _ = unresolved;
                    out.push_str(&format!("{} {}\n", colored, p));
                }
                return Ok(());
            }
            _ => {}
        }
    }
    // generic record: one field per line
    let m = match v {
        Value::Record(m) => m,
        _ => unreachable!(),
    };
    let name_w = m.keys().map(|k| width(k)).max().unwrap_or(0);
    for (k, x) in m.iter() {
        out.push_str(&pad);
        out.push_str(&pal.dim(&pad_right(k, name_w)));
        out.push_str("  ");
        // lists print in block form under their key (§5.1)
        if matches!(x, Value::List(xs) if !xs.is_empty()) {
            out.push('\n');
            display_block(interp, x, pal, indent + name_w + 2, out)?;
        } else {
            let line = display_line(interp, x, pal)?;
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(())
}

fn indent_multiline(s: &str, indent: usize) -> String {
    if indent == 0 {
        return s.to_string();
    }
    let pad = " ".repeat(indent);
    s.lines()
        .map(|l| format!("{}{}", pad, l))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn display_commit_block(
    interp: &mut Interp,
    c: &Value,
    pal: &Palette,
    indent: usize,
    out: &mut String,
) -> Result<(), Crash> {
    let pad = " ".repeat(indent);
    out.push_str(&pad);
    out.push_str(&commit_line(interp, c, pal, true, &BTreeSet::new(), false)?);
    out.push('\n');
    // author · age · n files — from metadata by id (§5.1)
    let id = match c.field("id")? {
        Value::Id(i) => i.to_string(),
        _ => String::new(),
    };
    let files = c.field("files").and_then(|v| v.as_list().map(|x| x.to_vec()))?;
    if let Ok(meta) = interp.backend.meta(&id) {
        let age = render_age(meta.time);
        out.push_str(&pad);
        out.push_str(&format!(
            "  {} · {} · {} files\n",
            meta.author,
            age,
            files.len()
        ));
    }
    for e in files.iter() {
        let path = path_string(&e.field("path")?)?;
        let content = e.field("content")?;
        let unresolved = matches!(&content, Value::Blob(b) if b.is_unresolved());
        out.push_str(&pad);
        if unresolved {
            out.push_str(&format!("  {} {}\n", pal.red("✖"), path));
        } else {
            out.push_str(&format!("    {}\n", path));
        }
    }
    Ok(())
}

pub fn render_age(time: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = (now - time).max(0);
    if d < 60 {
        format!("{}s", d)
    } else if d < 3600 {
        format!("{}m", d / 60)
    } else if d < 86400 {
        format!("{}h", d / 3600)
    } else if d < 86400 * 7 {
        format!("{}d", d / 86400)
    } else if d < 86400 * 365 {
        format!("{}w", d / (86400 * 7))
    } else {
        format!("{}y", d / (86400 * 365))
    }
}

/// the repo value in context (the loaded repo), for resolving ids to commits
fn top_of(interp: &Interp, _x: &Value) -> Result<Value, Crash> {
    let old = interp.old_repo.borrow().clone();
    old.ok_or_else(|| Crash::new("display: no repository in context"))
}

fn display_list_block(
    interp: &mut Interp,
    xs: &[Value],
    pal: &Palette,
    indent: usize,
    out: &mut String,
) -> Result<(), Crash> {
    let pad = " ".repeat(indent);
    if xs.is_empty() {
        out.push_str(&pad);
        out.push_str(&pal.dim("none"));
        out.push('\n');
        return Ok(());
    }
    // list of Id: commit table (one commit line per id, in list order)
    if xs.iter().all(|x| matches!(x, Value::Id(_))) {
        for x in xs {
            let id = match x {
                Value::Id(i) => i.to_string(),
                _ => unreachable!(),
            };
            out.push_str(&pad);
            let repo = top_of(interp, x)?;
            match crate::repo::by_id(&repo, &id)? {
                Some(loc) => {
                    let c = loc.field("root")?;
                    let conflict = has_conflict(&c)?;
                    let glyph = if conflict {
                        pal.red("⊗")
                    } else {
                        "○".to_string()
                    };
                    let msg = c.field("message")?.as_text()?.to_string();
                    let msg_first = msg.lines().next().unwrap_or("");
                    let labels: Vec<String> = c
                        .field("labels")?
                        .as_list()?
                        .iter()
                        .map(|l| l.as_text().map(|s| s.to_string()))
                        .collect::<Result<_, _>>()?;
                    let id_s = if conflict {
                        pal.red(&interp.backend.unique_prefix(&id))
                    } else {
                        display_id(interp, &id, pal)
                    };
                    if labels.is_empty() {
                        out.push_str(&format!("{} {}  {}\n", glyph, id_s, msg_first));
                    } else {
                        out.push_str(&format!(
                            "{} {}  {}  {}\n",
                            glyph,
                            id_s,
                            msg_first,
                            pal.green(&labels.join("  "))
                        ));
                    }
                }
                None => {
                    out.push_str(&display_id(interp, &id, pal));
                    out.push('\n');
                }
            }
        }
        return Ok(());
    }
    // list of Text
    if xs.iter().all(|x| matches!(x, Value::Text(_))) {
        for x in xs {
            out.push_str(&pad);
            out.push_str(x.as_text()?);
            out.push('\n');
        }
        return Ok(());
    }
    // list of [Text]: paths
    let all_path = xs.iter().all(|x| match x {
        Value::List(p) => p.iter().all(|c| matches!(c, Value::Text(_))),
        _ => false,
    });
    if all_path {
        for x in xs {
            out.push_str(&pad);
            out.push_str(&path_string(x)?);
            out.push('\n');
        }
        return Ok(());
    }
    // list of records with identical field sets: Entry list gets entry lines,
    // other identical sets get a column table
    let all_record = xs.iter().all(|x| matches!(x, Value::Record(_)));
    if all_record {
        let first = field_set(&xs[0]).unwrap();
        let same = xs
            .iter()
            .all(|x| field_set(x).map(|s| s == first).unwrap_or(false));
        if same {
            if shape_of_record(interp, &xs[0]).as_deref() == Some("Entry") {
                let width_max = xs
                    .iter()
                    .map(|x| path_string(&x.field("path").unwrap()).map(|p| width(&p)))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max()
                    .unwrap_or(0);
                for x in xs {
                    let path = path_string(&x.field("path")?)?;
                    let content = x.field("content")?;
                    let (size, unresolved) = match &content {
                        Value::Blob(b) => (human_size(b.size()), b.is_unresolved()),
                        _ => (String::new(), false),
                    };
                    out.push_str(&pad);
                    let mark = if unresolved { "✖" } else { " " };
                    let size_s = if unresolved {
                        pal.red(&size)
                    } else {
                        size
                    };
                    out.push_str(&format!("{} {:width$}  {}\n", mark, path, size_s, width = width_max));
                }
                return Ok(());
            }
            return display_table(interp, xs, pal, indent, out);
        }
    }
    // list of other scalars
    let all_scalar = xs
        .iter()
        .all(|x| matches!(x, Value::Int(_) | Value::Bool(_)));
    if all_scalar {
        out.push_str(&pad);
        out.push_str(&list_line(interp, xs, pal)?);
        out.push('\n');
        return Ok(());
    }
    // one block per item, separated by a blank line
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        display_block(interp, x, pal, indent, out)?;
    }
    Ok(())
}

fn display_table(
    interp: &mut Interp,
    xs: &[Value],
    pal: &Palette,
    indent: usize,
    out: &mut String,
) -> Result<(), Crash> {
    let pad = " ".repeat(indent);
    let fields: Vec<String> = field_set(&xs[0]).unwrap().into_iter().collect();
    // cells
    let mut rows: Vec<Vec<String>> = Vec::new();
    for x in xs {
        let mut row = Vec::new();
        for f in &fields {
            let cell = match x.field(f)? {
                Value::Bool(true) => "✓".to_string(),
                Value::Bool(false) => String::new(),
                other => display_line(interp, &other, pal)?,
            };
            row.push(cell);
        }
        rows.push(row);
    }
    // drop columns empty in every row
    let keep: Vec<usize> = (0..fields.len())
        .filter(|&i| rows.iter().any(|r| !r[i].is_empty()))
        .collect();
    let widths: Vec<usize> = keep
        .iter()
        .map(|&i| {
            rows.iter()
                .map(|r| width(&r[i]))
                .max()
                .unwrap_or(0)
                .max(width(&fields[i]))
        })
        .collect();
    // header
    out.push_str(&pad);
    let header: Vec<String> = keep
        .iter()
        .zip(&widths)
        .map(|(&i, w)| pad_right(&fields[i], *w))
        .collect();
    out.push_str(&pal.dim(&header.join("   ")));
    out.push('\n');
    for r in &rows {
        out.push_str(&pad);
        let cells: Vec<String> = keep
            .iter()
            .zip(&widths)
            .map(|(&i, w)| pad_right(&r[i], *w))
            .collect();
        out.push_str(&cells.join("   "));
        out.push('\n');
    }
    Ok(())
}

// ----------------------------------------------------------------------
// treeWith (§7.11)
// ----------------------------------------------------------------------

pub struct TreeOptions {
    pub detail: i64,
    pub margin: bool,
    pub elide: bool,
    pub icons: bool,
    pub color: String,
}

fn default_tree_options(_interp: &Interp) -> Result<TreeOptions, Crash> {
    Ok(TreeOptions {
        detail: 1,
        margin: false,
        elide: true,
        icons: false,
        color: "auto".into(),
    })
}

pub fn tree_with(interp: &mut Interp, opts: &Value, repo: &Value) -> Result<Value, Crash> {
    let detail = match opts.field("detail")? {
        Value::Int(n) => n.to_string().parse::<i64>().unwrap_or(1),
        _ => return Err(Crash::new("treeWith: detail must be an Int")),
    };
    let margin = match opts.field("margin")? {
        Value::Bool(b) => b,
        _ => return Err(Crash::new("treeWith: margin must be a Bool")),
    };
    let elide = match opts.field("elide")? {
        Value::Bool(b) => b,
        _ => return Err(Crash::new("treeWith: elide must be a Bool")),
    };
    let icons = match opts.field("icons")? {
        Value::Bool(b) => b,
        _ => return Err(Crash::new("treeWith: icons must be a Bool")),
    };
    let color = opts.field("color")?.as_text()?.to_string();
    let pal = Palette {
        on: color_enabled(&color),
    };
    let o = TreeOptions {
        detail,
        margin,
        elide,
        icons,
        color,
    };
    let text = tree_render(interp, &o, repo, &pal, true)?;
    Ok(Value::text(text))
}

struct CommitInfo {
    #[allow(dead_code)]
    commit: Value,
    id: String,
    message: String,
    labels: Vec<String>,
    conflict: bool,
    empty: bool,
    immutable: bool,
    is_focus: bool,
    is_ancestor_of_focus: bool,
    meta: Option<crate::domain::MetaInfo>,
    size: Option<usize>, // lines added+removed against parent
    detail_marks: Vec<(String, char)>,
    children: Vec<CommitInfo>,
}

fn tree_render(
    interp: &mut Interp,
    opts: &TreeOptions,
    repo: &Value,
    pal: &Palette,
    with_focus: bool,
) -> Result<String, Crash> {
    // immutable set
    let immutable = crate::repo::compute_immutable(interp, repo).unwrap_or_default();
    let focus_id = match repo.field("root")?.field("id")? {
        Value::Id(i) => i.to_string(),
        _ => String::new(),
    };
    // ancestors of focus
    let mut anc: BTreeSet<String> = BTreeSet::new();
    let mut cur = repo.clone();
    loop {
        let id = match cur.field("root")?.field("id")? {
            Value::Id(i) => i.to_string(),
            _ => break,
        };
        anc.insert(id);
        let ctx = cur.field("context").and_then(|v| v.as_list().map(|x| x.to_vec()))?;
        if ctx.is_empty() {
            break;
        }
        cur = crate::repo::by_id(&cur, &id_of_frame_parent(&ctx[0])?)?.unwrap();
    }
    // build info tree from the top
    let top = crate::repo::by_id(repo, &top_id(repo)?)?.unwrap_or_else(|| repo.clone());
    let root_commit = top.field("root")?;
    let top_children = top.field("children")?;
    let root_info = build_info(
        interp,
        opts,
        &top,
        &root_commit,
        top_children.as_list()?,
        &immutable,
        &focus_id,
        &anc,
        None,
        with_focus,
    )?;
    // layout
    let mut lines: Vec<String> = Vec::new();
    let label_col_needed = has_labels(&root_info);
    render_node(&root_info, opts, pal, "", true, true, label_col_needed, &mut lines);
    Ok(lines.join("\n") + "\n")
}

fn id_of_frame_parent(frame: &Value) -> Result<String, Crash> {
    match frame.field("parent")?.field("id")? {
        Value::Id(i) => Ok(i.to_string()),
        v => Err(Crash::new(format!("expected an Id, got a {}", v.kind_name()))),
    }
}

fn top_id(repo: &Value) -> Result<String, Crash> {
    let mut cur = repo.clone();
    loop {
        let ctx = cur.field("context").and_then(|v| v.as_list().map(|x| x.to_vec()))?;
        if ctx.is_empty() {
            return match cur.field("root")?.field("id")? {
                Value::Id(i) => Ok(i.to_string()),
                v => Err(Crash::new(format!("expected an Id, got a {}", v.kind_name()))),
            };
        }
        let pid = id_of_frame_parent(&ctx[0])?;
        cur = crate::repo::by_id(&cur, &pid)?.unwrap();
    }
}

#[allow(clippy::too_many_arguments)]
fn build_info(
    interp: &mut Interp,
    opts: &TreeOptions,
    loc: &Value,
    commit: &Value,
    children: &[Value],
    immutable: &BTreeSet<String>,
    focus_id: &str,
    anc: &BTreeSet<String>,
    parent_files: Option<&Value>,
    with_focus: bool,
) -> Result<CommitInfo, Crash> {
    let id = match commit.field("id")? {
        Value::Id(i) => i.to_string(),
        _ => String::new(),
    };
    let message = commit.field("message")?.as_text()?.to_string();
    let labels: Vec<String> = commit
        .field("labels")?
        .as_list()?
        .iter()
        .map(|l| l.as_text().map(|s| s.to_string()))
        .collect::<Result<_, _>>()?;
    let conflict = has_conflict(commit)?;
    let files = commit.field("files")?;
    let (empty, size, detail_marks) = match parent_files {
        Some(pf) => {
            let change = Value::record(&[("from", pf.clone()), ("to", files.clone())]);
            let marks = touched_paths(&change)?;
            let empty = marks.is_empty();
            let mut adds = 0usize;
            // size: lines added+removed against the parent
            for (_, m, _) in &marks {
                match m {
                    '+' | '−' => adds += 1,
                    _ => adds += 1,
                }
            }
            let marks2: Vec<(String, char)> =
                marks.iter().map(|(p, m, _)| (p.clone(), *m)).collect();
            (empty, Some(adds), marks2)
        }
        None => {
            let n = files.as_list()?.len();
            (n == 0 && id != ROOT_ID, None, Vec::new())
        }
    };
    let meta = interp.backend.meta(&id).ok();
    let is_focus = with_focus && id == focus_id;
    let mut infos = Vec::new();
    for c in children.iter() {
        let child_loc = crate::repo::by_id(loc, &match c.field("root")?.field("id")? {
            Value::Id(i) => i.to_string(),
            _ => continue,
        })?
        .unwrap();
        infos.push(build_info(
            interp,
            opts,
            &child_loc,
            &c.field("root")?,
            c.field("children")?.as_list()?,
            immutable,
            focus_id,
            anc,
            Some(&files),
            with_focus,
        )?);
    }
    let immutable_flag = immutable.contains(&id);
    let anc_flag = anc.contains(&id);
    Ok(CommitInfo {
        commit: commit.clone(),
        id,
        message,
        labels,
        conflict,
        empty,
        immutable: immutable_flag,
        is_focus,
        is_ancestor_of_focus: anc_flag,
        meta,
        size,
        detail_marks,
        children: infos,
    })
}

fn has_labels(info: &CommitInfo) -> bool {
    if !info.labels.is_empty() {
        return true;
    }
    info.children.iter().any(has_labels)
}

fn glyph_for(info: &CommitInfo, icons: bool) -> &'static str {
    if !icons {
        if info.id == ROOT_ID {
            "⌂"
        } else if info.conflict {
            "⊗"
        } else if info.empty {
            "◌"
        } else if info.immutable {
            "◆"
        } else if info.is_focus {
            "◉"
        } else if info.is_ancestor_of_focus {
            "●"
        } else {
            "○"
        }
    } else if info.id == ROOT_ID {
        "🌱"
    } else if info.conflict {
        "🔥"
    } else if info.empty {
        "🫙"
    } else if info.immutable {
        "🪨"
    } else if info.is_focus {
        "🌸"
    } else if info.is_ancestor_of_focus {
        "🌿"
    } else {
        "🍃"
    }
}

fn size_bar(size: Option<usize>, pal: &Palette) -> String {
    match size {
        None | Some(0) => String::new(),
        Some(n) => {
            let (bar, level) = if n >= 1000 {
                ("▇", 4)
            } else if n >= 200 {
                ("▇", 3)
            } else if n >= 50 {
                ("▅", 2)
            } else if n >= 10 {
                ("▃", 1)
            } else {
                ("▂", 0)
            };
            pal.accent(level, bar)
        }
    }
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_lowercase()
}

#[allow(clippy::too_many_arguments)]
fn render_node(
    info: &CommitInfo,
    opts: &TreeOptions,
    pal: &Palette,
    prefix: &str,
    last: bool,
    is_root: bool,
    label_col: bool,
    lines: &mut Vec<String>,
) {
    let gutter = if info.is_focus { "▶ " } else { "  " };
    let rail = if is_root {
        ""
    } else if last {
        "└─ "
    } else {
        "├─ "
    };
    let glyph = glyph_for(info, opts.icons);
    // id: shortest unique prefix, min 4 — needs backend; caller passes
    // preformatted? We keep full id short form here via stored prefix.
    let id_disp = info_display_id(info, pal);
    let bar = if opts.detail >= 1 {
        // focus and parent and children at detail 1; all at 2
        if opts.detail >= 2 || info.is_focus {
            size_bar(info.size, pal)
        } else {
            size_bar(info.size, pal)
        }
    } else {
        String::new()
    };
    let msg_first = info.message.lines().next().unwrap_or("");
    let msg = if info.empty {
        pal.dim_italic(msg_first)
    } else if info.is_focus {
        msg_first.to_string()
    } else if !info.is_ancestor_of_focus {
        msg_first.to_string()
    } else {
        msg_first.to_string()
    };
    let mut line = format!("{}{}{} {} {}", gutter, prefix, rail, glyph, id_disp);
    if !bar.is_empty() {
        line.push_str(&format!(" {}", bar));
    }
    if !msg.is_empty() {
        line.push_str(&format!("  {}", msg));
    }
    if label_col && !info.labels.is_empty() {
        line.push_str(&format!("  {}", pal.green(&info.labels.join("  "))));
    }
    if opts.margin {
        if let Some(meta) = &info.meta {
            let age = render_age(meta.time);
            let init = initials(&meta.author);
            line.push_str(&format!("  {}  {}", pal.grey(4, &age), pal.author(&meta.author, &init)));
        }
    }
    lines.push(line);
    // detail line (detail = 2, focus only)
    if opts.detail >= 2 && info.is_focus && !info.detail_marks.is_empty() {
        let cont = if is_root { "     " } else if last { "      " } else { "│     " };
        let marks: Vec<String> = info
            .detail_marks
            .iter()
            .map(|(p, m)| {
                let ms = m.to_string();
                let c = match m {
                    '+' => pal.green(&ms),
                    '−' => pal.red(&ms),
                    '~' => pal.yellow(&ms),
                    '✖' => pal.red(&ms),
                    _ => ms,
                };
                format!("{} {}", c, p)
            })
            .collect();
        lines.push(format!("  {}{}{}", prefix, cont, marks.join("   ")));
    }
    let child_prefix = if is_root {
        String::new()
    } else if last {
        format!("{}   ", prefix)
    } else {
        format!("{}│  ", prefix)
    };
    let n = info.children.len();
    for (i, c) in info.children.iter().enumerate() {
        render_node(c, opts, pal, &child_prefix, i == n - 1, false, label_col, lines);
    }
}

fn info_display_id(info: &CommitInfo, pal: &Palette) -> String {
    // prefix is precomputed into the commit id string by the caller when a
    // backend is available; here we use the first 4 chars as the unique part
    let id = &info.id;
    let n = 4.min(id.len());
    let p = &id[..n];
    let rest = &id[n..];
    let _ = rest;
    if info.conflict {
        pal.red(p)
    } else if info.immutable {
        pal.blue(p)
    } else {
        p.to_string()
    }
}

/// A reference to a commit that may or may not have stored metadata.
/// Minted commits (dry runs) carry no metadata (§7.11).
pub struct MetaLookup<'a> {
    pub interp: &'a Interp,
}

impl<'a> MetaLookup<'a> {
    pub fn get(&self, id: &str) -> Option<crate::domain::MetaInfo> {
        self.interp.backend.meta(id).ok()
    }
}
