//! Shape computation from typedecls (§4.12) and runtime contracts (§4.13).

use crate::ast::TypeExpr;
use crate::value::{PrimKind, ShapeKind, ShapeVal, Value};
use std::collections::{BTreeSet, HashMap};

#[derive(Default, Clone)]
pub struct Shapes {
    /// name -> its declared type
    pub decls: HashMap<String, TypeExpr>,
}

impl Shapes {
    pub fn new() -> Shapes {
        Shapes {
            decls: HashMap::new(),
        }
    }

    /// Compute the ShapeVal for a type name used as a value (§4.12).
    /// Returns None if the name is undeclared, or its shape is a function,
    /// a list, or otherwise not usable.
    pub fn shape_of(&self, name: &str) -> Option<ShapeVal> {
        match name {
            "Int" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Int),
            }),
            "Text" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Text),
            }),
            "Bool" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Bool),
            }),
            "Id" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Id),
            }),
            "Blob" => Some(ShapeVal {
                name: name.into(),
                kind: ShapeKind::Prim(PrimKind::Blob),
            }),
            "Shape" => None, // Shape itself is primitive but extract Shape is odd; crash
            _ => {
                let ty = self.decls.get(name)?;
                self.shape_of_type(ty).map(|kind| ShapeVal {
                    name: name.into(),
                    kind,
                })
            }
        }
    }

    fn shape_of_type(&self, ty: &TypeExpr) -> Option<ShapeKind> {
        match ty {
            TypeExpr::Con(n) => {
                if is_primitive(n) {
                    let s = self.shape_of(n)?;
                    Some(s.kind)
                } else if let Some(decl) = self.decls.get(n) {
                    // resolve the alias now that all decls are present
                    self.shape_of_type(decl)
                } else {
                    Some(ShapeKind::Aliased(n.clone()))
                }
            }
            TypeExpr::Record(fields) => Some(ShapeKind::Record(
                fields.iter().map(|(n, _)| n.clone()).collect(),
            )),
            TypeExpr::List(_) | TypeExpr::Fun(_, _) => None,
        }
    }

    /// Unfold an alias chain, owned, so it works with partially-populated
    /// decl maps too.
    pub fn unfold_owned(&self, ty: &TypeExpr) -> TypeExpr {
        match ty {
            TypeExpr::Con(n) => {
                if is_primitive(n) || is_type_var(n) {
                    return ty.clone();
                }
                match self.decls.get(n) {
                    Some(decl) => self.unfold_owned(decl),
                    None => ty.clone(),
                }
            }
            _ => ty.clone(),
        }
    }
}

fn is_primitive(n: &str) -> bool {
    matches!(n, "Int" | "Text" | "Bool" | "Id" | "Blob" | "Shape")
}

fn is_type_var(n: &str) -> bool {
    n.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
}

/// A compiled signature: a chain of parameter types and a final result type.
#[derive(Clone, Debug)]
pub struct Contract {
    pub params: Vec<TypeExpr>,
    pub result: TypeExpr,
}

pub fn compile_contract(shapes: &Shapes, ty: &TypeExpr) -> Contract {
    let mut params = Vec::new();
    let mut cur = ty;
    loop {
        match cur {
            TypeExpr::Fun(a, b) => {
                params.push((**a).clone());
                cur = b;
            }
            TypeExpr::Con(n) => {
                // alias of a function type is unfolded (§4.13)
                if !is_primitive(n) && !is_type_var(n) {
                    if let Some(decl) = shapes.decls.get(n) {
                        cur = decl;
                        continue;
                    }
                }
                break;
            }
            _ => break,
        }
    }
    Contract {
        params,
        result: cur.clone(),
    }
}

/// Check one level deep (§4.13). On failure, describe expected and got.
pub fn check(shapes: &Shapes, ty: &TypeExpr, v: &Value) -> Result<(), String> {
    let owned = shapes.unfold_owned(ty);
    let ty = &owned;
    match ty {
        TypeExpr::Con(n) => {
            if is_type_var(n) {
                return Ok(());
            }
            match n.as_str() {
                "Int" => kind_check(matches!(v, Value::Int(_)), "Int", v),
                "Text" => kind_check(matches!(v, Value::Text(_)), "Text", v),
                "Bool" => kind_check(matches!(v, Value::Bool(_)), "Bool", v),
                "Id" => kind_check(matches!(v, Value::Id(_)), "Id", v),
                "Blob" => kind_check(matches!(v, Value::Blob(_)), "Blob", v),
                "Shape" => kind_check(matches!(v, Value::Shape(_)), "Shape", v),
                _ => {
                    // shape name: exact field set (aliases resolved by shape_of)
                    match shapes.shape_of(n) {
                        Some(ShapeVal {
                            kind: ShapeKind::Record(fields),
                            ..
                        }) => record_check(&fields, n, v),
                        Some(ShapeVal {
                            kind: ShapeKind::Prim(p),
                            ..
                        }) => {
                            let ok = matches!(
                                (p, v),
                                (PrimKind::Int, Value::Int(_))
                                    | (PrimKind::Text, Value::Text(_))
                                    | (PrimKind::Bool, Value::Bool(_))
                                    | (PrimKind::Id, Value::Id(_))
                                    | (PrimKind::Blob, Value::Blob(_))
                            );
                            kind_check(ok, n, v)
                        }
                        _ => Ok(()),
                    }
                }
            }
        }
        TypeExpr::List(_) => kind_check(matches!(v, Value::List(_)), "a list", v),
        TypeExpr::Record(fields) => {
            let set: BTreeSet<String> = fields.iter().map(|(n, _)| n.clone()).collect();
            record_check(&set, "record", v)
        }
        TypeExpr::Fun(_, _) => kind_check(matches!(v, Value::Fun(_)), "a function", v),
    }
}

fn kind_check(ok: bool, expected: &str, v: &Value) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("expected {}, got {}", expected, v.kind_name()))
    }
}

fn record_check(expected: &BTreeSet<String>, name: &str, v: &Value) -> Result<(), String> {
    match v {
        Value::Record(m) => {
            let got: BTreeSet<String> = m.keys().cloned().collect();
            if *expected == got {
                Ok(())
            } else {
                Err(format!(
                    "expected {} {}, got {}",
                    if name == "record" { "" } else { name },
                    fmt_fields(expected),
                    fmt_fields(&got)
                ))
            }
        }
        _ => Err(format!("expected {}, got {}", name, v.kind_name())),
    }
}

pub fn fmt_fields(s: &BTreeSet<String>) -> String {
    format!("{{ {} }}", s.iter().cloned().collect::<Vec<_>>().join(", "))
}

/// A human-readable name for a type, used in contract messages.
pub fn type_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::Con(n) => n.clone(),
        TypeExpr::List(t) => format!("[{}]", type_name(t)),
        TypeExpr::Record(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(n, t)| format!("{} : {}", n, type_name(t)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        TypeExpr::Fun(a, b) => format!("{} -> {}", type_name(a), type_name(b)),
    }
}

// ----------------------------------------------------------------------
// contract expressions: the type of a (possibly partially applied) function
// ----------------------------------------------------------------------

use std::rc::Rc as Rc2;

#[derive(Clone, Debug)]
pub enum ContractExpr {
    /// no contract information; arg checks pass through
    Unknown,
    Known(Rc2<Contract>),
    /// compose(f, g): (f . g); both contract expressions
    Compose(Box<ContractExpr>, Box<ContractExpr>),
    /// apply e to an argument that checked against e's first parameter
    ApplyFirst(Box<ContractExpr>),
}

impl ContractExpr {
    /// The type of the next parameter this function expects, if known.
    pub fn next_param(&self) -> Option<TypeExpr> {
        match self {
            ContractExpr::Unknown => None,
            ContractExpr::Known(c) => c.params.first().cloned(),
            ContractExpr::Compose(f, g) => g
                .next_param()
                .or_else(|| f.next_param())
                .or_else(|| f.result_after()),
            ContractExpr::ApplyFirst(e) => {
                let advanced = ContractExpr::apply_first(e.clone());
                advanced.next_param()
            }

        }
    }

    /// The result type once all parameters are supplied.
    pub fn result_after(&self) -> Option<TypeExpr> {
        match self {
            ContractExpr::Unknown => None,
            ContractExpr::Known(c) => {
                if c.params.is_empty() {
                    Some(c.result.clone())
                } else {
                    None
                }
            }
            ContractExpr::Compose(f, g) => {
                if g.is_exhausted() {
                    f.result_after()
                } else {
                    None
                }
            }
            ContractExpr::ApplyFirst(_) => None,
        }
    }

    /// True when no parameters remain (value-level).
    pub fn is_exhausted(&self) -> bool {
        match self {
            ContractExpr::Unknown => false,
            ContractExpr::Known(c) => c.params.is_empty(),
            ContractExpr::Compose(_, g) => g.is_exhausted(),
            ContractExpr::ApplyFirst(_) => false,
        }
    }

    /// Advance after one argument has been supplied and checked.
    pub fn apply_first_rc(self: Rc2<ContractExpr>) -> ContractExpr {
        ContractExpr::apply_first(Box::new((*self).clone()))
    }

    /// Advance after one argument has been supplied and checked.
    pub fn apply_first(self: Box<ContractExpr>) -> ContractExpr {
        match *self {
            ContractExpr::Unknown => ContractExpr::Unknown,
            ContractExpr::Known(c) => {
                let mut c2 = (*c).clone();
                if !c2.params.is_empty() {
                    c2.params.remove(0);
                }
                ContractExpr::Known(Rc2::new(c2))
            }
            ContractExpr::Compose(f, g) => {
                if !g.is_exhausted() {
                    ContractExpr::Compose(f, Box::new(ContractExpr::apply_first(g)))
                } else {
                    // x flowed into f
                    ContractExpr::ApplyFirst(f)
                }
            }
            ContractExpr::ApplyFirst(e) => ContractExpr::ApplyFirst(Box::new(ContractExpr::apply_first(e))),
        }
    }

    /// Number of remaining parameters, if known and not nested.
    pub fn params_len(&self) -> usize {
        match self {
            ContractExpr::Known(c) => c.params.len(),
            _ => 99,
        }
    }

    /// True if the next parameter is definitely a function type.
    pub fn next_param_is_function(&self, shapes: &Shapes) -> bool {
        match self.next_param() {
            Some(t) => matches!(shapes.unfold_owned(&t), TypeExpr::Fun(_, _)),
            None => false,
        }
    }

    /// Check an argument to this function. The outer ApplyFirst layer (if
    /// any) accounts for the argument currently being consumed.
    pub fn check_arg(&self, shapes: &Shapes, fname: &str, v: &Value) -> Result<(), Crash2> {
        self.check_arg_at(shapes, fname, 0, v)
    }

    /// like check_arg, with the 0-based position of the argument for messages
    pub fn check_arg_at(
        &self,
        shapes: &Shapes,
        fname: &str,
        position: usize,
        v: &Value,
    ) -> Result<(), Crash2> {
        let peeled = match self {
            ContractExpr::ApplyFirst(e) => ContractExpr::apply_first(e.clone()),
            other => other.clone(),
        };
        if let Some(param) = peeled.next_param() {
            if let Err(msg) = check(shapes, &param, v) {
                // `check` reports "expected T, got V". Show the argument
                // position with the declared type, then the got-kind; when the
                // declared type is a type variable or alias whose inner
                // description adds information (e.g. "[a]" vs "a list"), the
                // inner "expected" is more helpful, so prefer it (§4.13).
                let (exp, got) = match msg.split_once(", got ") {
                    Some((e, g)) => (
                        e.strip_prefix("expected ").unwrap_or(e).to_string(),
                        g.to_string(),
                    ),
                    None => (type_name(&param), v.kind_name().to_string()),
                };
                let declared = type_name(&param);
                let shown = if declared == exp { declared } else { format!("{} ({})", declared, exp) };
                return Err(Crash2::new(format!(
                    "contract: {} expected {} as argument {}, got {}",
                    fname,
                    shown,
                    position + 1,
                    got
                )));
            }
        }
        Ok(())
    }

    /// Check a result. Only called when this expression is exhausted.
    pub fn check_result(&self, shapes: &Shapes, fname: &str, v: &Value) -> Result<(), Crash2> {
        if !self.is_exhausted() {
            // more parameters remain: the result must be a function; further
            // checking happens when it is applied
            if !matches!(v, Value::Fun(_)) {
                return Err(Crash2::new(format!(
                    "contract: {} expected a function, got {}",
                    fname,
                    v.kind_name()
                )));
            }
            return Ok(());
        }
        if let Some(res) = self.result_after() {
            if let Err(msg) = check(shapes, &res, v) {
                return Err(Crash2::new(format!(
                    "contract: {}: result {}",
                    fname, msg
                )));
            }
        }
        Ok(())
    }
}

pub type Crash2 = crate::value::Crash;
