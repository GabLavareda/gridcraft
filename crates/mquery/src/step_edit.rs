//! Source ranges of references to a top-level step. Scope resolution prevents changing fields,
//! strings, comments or local bindings that happen to have the same name.
use crate::ast::*;

pub fn references(source: &str, from: &str, to: &str) -> Result<Vec<(usize, usize)>, String> {
    let tree = crate::parse(source).map_err(|e| e.message)?;
    let Expr::Let(bindings, _) = &tree else {
        return Err("The formula has no applied steps.".into());
    };
    if bindings.index_of(from).is_none() {
        return Err(format!("There is no step named '{from}'."));
    }
    let mut out = Vec::new();
    walk(&tree, from, to, &mut Vec::new(), &mut out)?;
    Ok(out)
}

fn walk<'a>(e: &'a Expr, from: &str, to: &str, scopes: &mut Vec<Vec<&'a str>>, out: &mut Vec<(usize, usize)>) -> Result<(), String> {
    match e {
        Expr::Ident { name, .. } if from != to && &**name == to && !scopes.iter().any(|s| s.contains(&to)) => {
            return Err(format!("Renaming would capture an existing reference to '{to}'."));
        }
        Expr::Ident { name, span, skip, .. } if &**name == from => {
            let mut skip = *skip;
            for (index, scope) in scopes.iter().enumerate().rev() {
                if !scope.contains(&from) {
                    continue;
                }
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                if index == 0 {
                    if scopes.iter().skip(1).any(|s| s.contains(&to)) {
                        return Err(format!("The name '{to}' is already used in a nested scope."));
                    }
                    out.push(*span);
                }
                break;
            }
        }
        Expr::Let(b, body) => {
            scopes.push(b.names.iter().map(|n| &**n).collect());
            for e in &b.exprs {
                walk(e, from, to, scopes, out)?;
            }
            walk(body, from, to, scopes, out)?;
            scopes.pop();
        }
        Expr::Record(b) => {
            scopes.push(b.names.iter().map(|n| &**n).collect());
            for e in &b.exprs {
                walk(e, from, to, scopes, out)?;
            }
            scopes.pop();
        }
        Expr::Function(f) => function(f, from, to, scopes, out)?,
        Expr::List(items) => {
            for item in items {
                match item {
                    ListItem::One(e) => walk(e, from, to, scopes, out)?,
                    ListItem::Range(a, b) => {
                        walk(a, from, to, scopes, out)?;
                        walk(b, from, to, scopes, out)?;
                    }
                }
            }
        }
        Expr::Unary(_, e) | Expr::Raise(e) => walk(e, from, to, scopes, out)?,
        Expr::As(e, t) | Expr::Is(e, t) => {
            walk(e, from, to, scopes, out)?;
            ty(t, from, to, scopes, out)?;
        }
        Expr::Type(t) => ty(t, from, to, scopes, out)?,
        Expr::Chain(e, rest) => {
            walk(e, from, to, scopes, out)?;
            for (_, e) in rest {
                walk(e, from, to, scopes, out)?;
            }
        }
        Expr::If(c, a, b) => {
            walk(c, from, to, scopes, out)?;
            walk(a, from, to, scopes, out)?;
            walk(b, from, to, scopes, out)?;
        }
        Expr::Invoke(f, args) => {
            walk(f, from, to, scopes, out)?;
            for e in args {
                walk(e, from, to, scopes, out)?;
            }
        }
        Expr::Field { target, .. } | Expr::Project { target, .. } => {
            if let Some(e) = target {
                walk(e, from, to, scopes, out)?;
            }
        }
        Expr::Item { target, index, .. } => {
            walk(target, from, to, scopes, out)?;
            walk(index, from, to, scopes, out)?;
        }
        Expr::Try(e, h) => {
            walk(e, from, to, scopes, out)?;
            match h {
                TryHandler::None => {}
                TryHandler::Otherwise(e) => walk(e, from, to, scopes, out)?,
                TryHandler::Catch(f) => function(f, from, to, scopes, out)?,
            }
        }
        _ => {}
    }
    Ok(())
}

fn function<'a>(f: &'a FuncDecl, from: &str, to: &str, scopes: &mut Vec<Vec<&'a str>>, out: &mut Vec<(usize, usize)>) -> Result<(), String> {
    scopes.push(f.params.iter().map(|p| &*p.name).collect());
    walk(&f.body, from, to, scopes, out)?;
    scopes.pop();
    Ok(())
}

fn ty<'a>(t: &'a TypeExpr, from: &str, to: &str, scopes: &mut Vec<Vec<&'a str>>, out: &mut Vec<(usize, usize)>) -> Result<(), String> {
    match t {
        TypeExpr::Nullable(t) | TypeExpr::List(t) => ty(t, from, to, scopes, out)?,
        TypeExpr::Record(fields, _) | TypeExpr::Table(fields) => {
            for f in fields {
                if let Some(t) = &f.ty {
                    ty(t, from, to, scopes, out)?;
                }
            }
        }
        TypeExpr::Expr(e) | TypeExpr::TableOf(e) => walk(e, from, to, scopes, out)?,
        _ => {}
    }
    Ok(())
}
