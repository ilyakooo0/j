//! Values of the j language (§2).

use crate::ast::{Expr, Pattern};
use num_bigint::BigInt;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

pub type RecordMap = BTreeMap<String, Value>;

#[derive(Clone)]
pub enum Value {
    Int(BigInt),
    Text(Rc<String>),
    Bool(bool),
    List(Rc<Vec<Value>>),
    Record(Rc<RecordMap>),
    Fun(Rc<FunVal>),
    Id(Rc<String>),
    Blob(Rc<BlobVal>),
    Shape(Rc<ShapeVal>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlobKind {
    Regular,
    Executable,
    Symlink,
}

#[derive(Clone, Debug)]
pub enum BlobContent {
    Resolved(Rc<Vec<u8>>),
    /// Conflict sides, jj order: alternating adds and removes, starting and
    /// ending with an add: [add, (remove, add)*]. For the in-memory backend:
    /// [to, from, onto]-style three sides as [add, remove, add].
    Conflict(Vec<Rc<Vec<u8>>>),
}

#[derive(Clone, Debug)]
pub struct BlobVal {
    pub kind: BlobKind,
    pub content: BlobContent,
}

impl BlobVal {
    pub fn text_blob(s: &str) -> Value {
        Value::Blob(Rc::new(BlobVal {
            kind: BlobKind::Regular,
            content: BlobContent::Resolved(Rc::new(s.as_bytes().to_vec())),
        }))
    }
    pub fn is_unresolved(&self) -> bool {
        matches!(self.content, BlobContent::Conflict(_))
    }
    pub fn size(&self) -> usize {
        match &self.content {
            BlobContent::Resolved(b) => b.len(),
            BlobContent::Conflict(sides) => sides.iter().map(|s| s.len()).sum(),
        }
    }
    /// content as bytes; conflicts render with jj-style conflict markers
    pub fn bytes(&self) -> Vec<u8> {
        match &self.content {
            BlobContent::Resolved(b) => b.as_ref().clone(),
            BlobContent::Conflict(sides) => render_conflict(sides),
        }
    }
}

/// Render a conflict with jj-style markers.
pub fn render_conflict(sides: &[Rc<Vec<u8>>]) -> Vec<u8> {
    // sides: [add0, remove0, add1, remove1, ... addN]
    let mut out = Vec::new();
    out.extend_from_slice(b"<<<<<<<\n");
    let mut i = 0;
    let mut first_add = true;
    while i < sides.len() {
        if i % 2 == 0 {
            if !first_add {
                // another add after removes: separate
            }
            out.extend_from_slice(b"+++++++\n");
            out.extend_from_slice(&sides[i]);
            ensure_newline(&mut out);
            first_add = false;
        } else {
            out.extend_from_slice(b"%%%%%%%\n");
            out.extend_from_slice(&sides[i]);
            ensure_newline(&mut out);
        }
        i += 1;
    }
    out.extend_from_slice(b">>>>>>>\n");
    out
}

fn ensure_newline(v: &mut Vec<u8>) {
    if !v.is_empty() && !v.ends_with(b"\n") {
        v.push(b'\n');
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrimKind {
    Int,
    Text,
    Bool,
    Id,
    Blob,
}

impl PrimKind {
    pub fn name(&self) -> &'static str {
        match self {
            PrimKind::Int => "Int",
            PrimKind::Text => "Text",
            PrimKind::Bool => "Bool",
            PrimKind::Id => "Id",
            PrimKind::Blob => "Blob",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ShapeKind {
    Record(BTreeSet<String>),
    Prim(PrimKind),
    /// an alias that could not be resolved to a record shape yet (function,
    /// list, or a typedecl processed later); resolved lazily at check time
    Aliased(String),
}

#[derive(Clone, Debug)]
pub struct ShapeVal {
    pub name: String,
    pub kind: ShapeKind,
}

impl PartialEq for ShapeVal {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

pub type BuiltinFn = fn(&mut crate::eval::Interp, &[Value]) -> Result<Value, Crash>;

pub enum FunVal {
    Builtin {
        name: String,
        arity: usize,
        args: Vec<Value>,
        f: BuiltinFn,
        pending: Option<(String, Rc<crate::shape::ContractExpr>)>,
    },
    Closure {
        name: Option<String>, // top-level definition name, if any
        params: Vec<Pattern>,
        applied: usize,
        /// the arguments supplied so far, for rendering (§5.2)
        applied_args: Vec<Value>,
        /// the body has not been evaluated yet (the definition's contract is
        /// not exhausted): the next application evaluates it, then applies
        deferred: bool,
        body: Rc<Expr>,
        env: Env,
        src: String,
        /// contract of this function as further arguments arrive
        pending: Option<(String, Rc<crate::shape::ContractExpr>)>,
    },
    /// `f or g` lifted pointwise over functions (§4.6)
    OrFun(Value, Value),
    /// unevaluated composition: unfolds only when applied to a non-function
    /// (§4.1 note — `abandon . contract everything` is a value before it is
    /// applied, and must not run at load)
    ComposeLazy(Value, Value),
    /// a label literal %name: applies as `labelled "name"`
    Labelled(String, Value),
}

impl Value {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Text(_) => "Text",
            Value::Bool(_) => "Bool",
            Value::List(_) => "list",
            Value::Record(_) => "record",
            Value::Fun(_) => "function",
            Value::Id(_) => "Id",
            Value::Blob(_) => "Blob",
            Value::Shape(_) => "Shape",
        }
    }

    pub fn record(fields: &[(&str, Value)]) -> Value {
        let mut m = RecordMap::new();
        for (k, v) in fields {
            m.insert(k.to_string(), v.clone());
        }
        Value::Record(Rc::new(m))
    }

    pub fn field(&self, name: &str) -> Result<Value, Crash> {
        match self {
            Value::Record(m) => m
                .get(name)
                .cloned()
                .ok_or_else(|| Crash::new(format!("record has no field `{}`", name))),
            _ => Err(Crash::new(format!(
                "cannot select field `{}` from a {}",
                name,
                self.kind_name()
            ))),
        }
    }

    pub fn field_set(&self) -> Option<BTreeSet<String>> {
        match self {
            Value::Record(m) => Some(m.keys().cloned().collect()),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Result<&[Value], Crash> {
        match self {
            Value::List(xs) => Ok(xs),
            _ => Err(Crash::new(format!("expected a list, got a {}", self.kind_name()))),
        }
    }

    pub fn as_text(&self) -> Result<&str, Crash> {
        match self {
            Value::Text(t) => Ok(t),
            _ => Err(Crash::new(format!("expected Text, got a {}", self.kind_name()))),
        }
    }

    pub fn as_int(&self) -> Result<&BigInt, Crash> {
        match self {
            Value::Int(n) => Ok(n),
            _ => Err(Crash::new(format!("expected Int, got a {}", self.kind_name()))),
        }
    }

    pub fn text(s: impl Into<String>) -> Value {
        Value::Text(Rc::new(s.into()))
    }

    pub fn int(n: i64) -> Value {
        Value::Int(BigInt::from(n))
    }

    pub fn list(xs: Vec<Value>) -> Value {
        Value::List(Rc::new(xs))
    }

    pub fn bool(b: bool) -> Value {
        Value::Bool(b)
    }
}

/// Attach a definition name and contract to a function value (§4.13).
pub fn attach_pending(v: &Value, name: &str, contract: Rc<crate::shape::Contract>) -> Value {
    let pending = Some((
        name.to_string(),
        Rc::new(crate::shape::ContractExpr::Known(contract)),
    ));
    match v {
        Value::Fun(fv) => match fv.as_ref() {
            FunVal::Closure {
                params,
                applied,
                applied_args,
                deferred,
                body,
                env,
                src,
                ..
            } => Value::Fun(Rc::new(FunVal::Closure {
                name: Some(name.to_string()),
                params: params.clone(),
                applied: *applied,
                applied_args: applied_args.clone(),
                deferred: *deferred,
                body: body.clone(),
                env: env.clone(),
                src: src.clone(),
                pending,
            })),
            FunVal::Builtin {
                name: bname,
                arity,
                args,
                f,
                ..
            } => Value::Fun(Rc::new(FunVal::Builtin {
                name: bname.clone(),
                arity: *arity,
                args: args.clone(),
                f: *f,
                pending,
            })),
            FunVal::OrFun(_, _) | FunVal::ComposeLazy(_, _) | FunVal::Labelled(_, _) => v.clone(),
        },
        _ => v.clone(),
    }
}

/// A crash (§4.7).
#[derive(Debug, Clone)]
pub struct Crash {
    pub msg: String,
    /// innermost base definition executing, if any
    pub def: Option<String>,
}

impl Crash {
    pub fn new(msg: impl Into<String>) -> Crash {
        Crash {
            msg: msg.into(),
            def: None,
        }
    }
}

// ----------------------------------------------------------------------
// Environments (persistent, for closures)
// ----------------------------------------------------------------------

#[derive(Clone)]
pub struct Env {
    frame: Option<Rc<Frame>>,
}

pub struct Frame {
    kind: FrameKind,
    parent: Env,
}

enum FrameKind {
    Small(Vec<(String, Value)>),
    Big(HashMap<String, Value>),
    /// recursive let bindings: filled incrementally, read through the cell so
    /// closures capturing the frame see later bindings (§4.1)
    Rec(Rc<std::cell::RefCell<Vec<(String, Value)>>>),
}

impl Env {
    pub fn empty() -> Env {
        Env { frame: None }
    }

    pub fn with_globals(globals: HashMap<String, Value>) -> Env {
        Env {
            frame: Some(Rc::new(Frame {
                kind: FrameKind::Big(globals),
                parent: Env::empty(),
            })),
        }
    }

    pub fn extend(&self, bindings: Vec<(String, Value)>) -> Env {
        Env {
            frame: Some(Rc::new(Frame {
                kind: FrameKind::Small(bindings),
                parent: self.clone(),
            })),
        }
    }

    /// extend with a recursive frame (for `let` blocks)
    pub fn extend_rec(&self) -> (Env, Rc<std::cell::RefCell<Vec<(String, Value)>>>) {
        let cell = Rc::new(std::cell::RefCell::new(Vec::new()));
        (
            Env {
                frame: Some(Rc::new(Frame {
                    kind: FrameKind::Rec(cell.clone()),
                    parent: self.clone(),
                })),
            },
            cell,
        )
    }

    pub fn lookup(&self, name: &str) -> Option<Value> {
        let mut cur = self.frame.as_ref();
        while let Some(f) = cur {
            match &f.kind {
                FrameKind::Small(bs) => {
                    for (n, v) in bs.iter().rev() {
                        if n == name {
                            return Some(v.clone());
                        }
                    }
                }
                FrameKind::Rec(cell) => {
                    for (n, v) in cell.borrow().iter().rev() {
                        if n == name {
                            return Some(v.clone());
                        }
                    }
                }
                FrameKind::Big(m) => {
                    if let Some(v) = m.get(name) {
                        return Some(v.clone());
                    }
                }
            }
            cur = f.parent.frame.as_ref();
        }
        None
    }

    /// True if this environment captures anything beyond the globals frame
    /// (i.e. the closure was created inside a lambda/let body).
    pub fn is_closure_capture(&self) -> bool {
        let mut cur = self.frame.as_ref();
        let mut small_frames = 0;
        while let Some(f) = cur {
            match &f.kind {
                FrameKind::Small(_) => small_frames += 1,
                FrameKind::Big(_) | FrameKind::Rec(_) => {}
            }
            cur = f.parent.frame.as_ref();
        }
        small_frames > 0
    }

    pub fn names(&self, out: &mut BTreeSet<String>) {
        let mut cur = self.frame.as_ref();
        while let Some(f) = cur {
            match &f.kind {
                FrameKind::Small(bs) => {
                    for (n, _) in bs {
                        out.insert(n.clone());
                    }
                }
                FrameKind::Rec(cell) => {
                    for (n, _) in cell.borrow().iter() {
                        out.insert(n.clone());
                    }
                }
                FrameKind::Big(m) => {
                    for n in m.keys() {
                        out.insert(n.clone());
                    }
                }
            }
            cur = f.parent.frame.as_ref();
        }
    }
}

// ----------------------------------------------------------------------
// structural equality (§4.5); comparing functions is a crash
// ----------------------------------------------------------------------

pub fn value_eq(a: &Value, b: &Value) -> Result<bool, Crash> {
    Ok(match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Text(x), Value::Text(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Id(x), Value::Id(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            for (u, v) in x.iter().zip(y.iter()) {
                if !value_eq(u, v)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Record(x), Value::Record(y)) => {
            if x.len() != y.len() {
                return Ok(false);
            }
            for ((k1, v1), (k2, v2)) in x.iter().zip(y.iter()) {
                if k1 != k2 || !value_eq(v1, v2)? {
                    return Ok(false);
                }
            }
            true
        }
        (Value::Blob(x), Value::Blob(y)) => x.kind == y.kind && blob_content_eq(&x.content, &y.content),
        (Value::Shape(x), Value::Shape(y)) => x == y,
        (Value::Fun(_), Value::Fun(_)) => {
            return Err(Crash::new("cannot compare functions for equality"))
        }
        _ => false,
    })
}

fn blob_content_eq(a: &BlobContent, b: &BlobContent) -> bool {
    match (a, b) {
        (BlobContent::Resolved(x), BlobContent::Resolved(y)) => x == y,
        (BlobContent::Conflict(x), BlobContent::Conflict(y)) => x == y,
        _ => false,
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // cheap debug rendering without an interpreter
        match self {
            Value::Int(n) => write!(f, "Int({})", n),
            Value::Text(t) => write!(f, "Text({:?})", t),
            Value::Bool(b) => write!(f, "Bool({})", b),
            Value::Id(i) => write!(f, "Id(@{})", i),
            Value::List(xs) => f.debug_list().entries(xs.iter()).finish(),
            Value::Record(m) => f.debug_map().entries(m.iter()).finish(),
            Value::Fun(_) => write!(f, "Fun(..)"),
            Value::Blob(b) => write!(f, "Blob({:?})", b),
            Value::Shape(s) => write!(f, "Shape({})", s.name),
        }
    }
}
