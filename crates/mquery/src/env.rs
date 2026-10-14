//! Scopes: where names are looked up.

use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::{Bindings, FuncDecl};
use crate::error::MError;
use crate::value::{Deferred, Record, Slot, Value, defer, error_needs_defer};

pub struct Env {
    pub(crate) parent: Option<Rc<Env>>,
    pub(crate) frame: Frame,
}

pub(crate) enum Frame {
    /// The arguments of a function call.
    Args { decl: Rc<FuncDecl>, values: Vec<Value> },
    /// The members of a `let`, a record literal or the section: evaluated when first used.
    Lazy { decl: Rc<Bindings>, cells: Vec<RefCell<Thunk>> },
    /// The fields of a record (the environment given to `Expression.Evaluate`).
    Fields(Rc<Record>),
    /// Names don't resolve past this frame, not even library names.
    Barrier,
}

pub(crate) enum Thunk {
    Pending,
    /// Being evaluated: reaching it again is a cyclic reference.
    Running,
    Done(Value),
    Failed(Rc<MError>),
    /// A function defined directly in this scope. Kept without its scope, which would otherwise
    /// own itself through the function and never be freed.
    SelfFn(Rc<FuncDecl>),
}

impl Env {
    pub(crate) fn new(parent: Option<Rc<Env>>, frame: Frame) -> Rc<Env> {
        Rc::new(Env { parent, frame })
    }
    pub(crate) fn lazy(decl: &Rc<Bindings>, parent: Option<Rc<Env>>) -> Rc<Env> {
        let cells = decl.names.iter().map(|_| RefCell::new(Thunk::Pending)).collect();
        Env::new(parent, Frame::Lazy { decl: decl.clone(), cells })
    }
    /// Forgets what the scope's members evaluated to. This breaks the cycle between a scope and
    /// the functions it holds that captured it.
    pub(crate) fn reset(&self) {
        if let Frame::Lazy { cells, .. } = &self.frame {
            for c in cells {
                if let Ok(mut c) = c.try_borrow_mut() {
                    *c = Thunk::Pending;
                }
            }
        }
    }
}

// Scopes chain through the values they hold (a function holds its scope, which holds values…),
// so like the containers they hand their children to the drop queue.
impl Drop for Env {
    fn drop(&mut self) {
        match &mut self.frame {
            Frame::Args { values, .. } => {
                for v in values {
                    if v.needs_defer() {
                        defer(Deferred::Slot(Slot::Val(std::mem::replace(v, Value::Null))));
                    }
                }
            }
            Frame::Lazy { cells, .. } => {
                for c in cells {
                    let c = c.get_mut();
                    let owns = match c {
                        Thunk::Done(v) => v.needs_defer(),
                        Thunk::Failed(e) => error_needs_defer(e),
                        _ => false,
                    };
                    if owns {
                        match std::mem::replace(c, Thunk::Pending) {
                            Thunk::Done(v) => defer(Deferred::Slot(Slot::Val(v))),
                            Thunk::Failed(e) => defer(Deferred::Slot(Slot::Err(e))),
                            _ => {}
                        }
                    }
                }
            }
            Frame::Fields(_) | Frame::Barrier => {}
        }
        if let Some(p) = self.parent.take()
            && Rc::strong_count(&p) == 1
        {
            defer(Deferred::Env(p));
        }
    }
}
