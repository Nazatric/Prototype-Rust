//! vm — a safe-Rust Lua 5.1 instruction interpreter ("mini VM").
//!
//! ## Scope (M8)
//!
//! Executes the *data-shaping* subset of the recovered Prototype
//! scripts — table constructors, constants, arithmetic, generic-for
//! over `pairs`, calls into host (engine) functions, closures with
//! upvalues.  Validated against the real shipped scripts:
//!
//! * `frevents.lua` — builds the free-roam event definition set
//!   (60 event-def tables), registers them with the engine via
//!   `fre_RegisterEvent` / `fre_RegisterCollectibles`, and defines
//!   the `ResetFreeRoamEvents` / `DestroyFreeRoamEvents` lifecycle
//!   closures.  The full run reproduces the M5 static analysis
//!   exactly (see the `vm_frevents_*` tests).
//! * the 2318-byte mission script from `scripts.rcf` — calls
//!   `AddValidMission` exactly 70 times, matching the static
//!   GETGLOBAL census.
//!
//! ## PROVEN (byte evidence)
//!
//! * Instruction encoding: Lua 5.1 `Op(6) | A(8) | C(9) | B(9)`,
//!   Bx at bit 14 (18 bits), sBx biased by 131071 — the same decode
//!   used by [`crate::Instruction`], verified against real chunks.
//! * Register windows: callee `R[0]` = caller `R[A+1]` (Lua 5.1
//!   `ci->base = ci->func + 1`); parameters start at callee `R[0]`.
//! * `OP_CLOSURE` is followed by `nups` *pseudo-instructions*
//!   (OP_MOVE = capture stack slot, OP_GETUPVAL = capture parent
//!   upvalue) which are consumed, not executed.  PROVEN by the
//!   `CLOSURE/MOVE` pair at pcs 665/666 of `frevents.lua`.
//! * `LFIELDS_PER_FLUSH = 50` for OP_SETLIST — PROVEN by
//!   `SETLIST B=50 C=1` (indices 1..50) followed by
//!   `SETLIST B=10 C=2` (indices 51..60) in `frevents.lua`.
//!
//! ## LIKELY (spec-derived, not contradicted by any fixture)
//!
//! * TEST/TESTSET/EQ/LT/LE "skip the following JMP when the truth
//!   of the comparison differs from C" — from the Lua 5.1
//!   `lopcodes.h` comments.  No shipped Prototype script uses these
//!   opcodes yet (census-verified), so they are exercised here only
//!   by synthetic tests.
//! * Numeric-for semantics (`FORPREP` subtracts the step, `FORLOOP`
//!   adds it back before testing) — standard 5.1, exercised by a
//!   synthetic loop test.
//!
//! ## KNOWN SIMPLIFICATIONS (honest limits)
//!
//! * **Upvalues**: a stack capture (`MOVE` pseudo) snapshots the
//!   register into a fresh `Rc<RefCell<Value>>` at CLOSURE time.
//!   True Lua 5.1 keeps the upvalue *open* (aliasing the live stack
//!   slot) until the frame returns.  The two differ only when a
//!   captured local is *reassigned* after the CLOSURE instruction —
//!   never the case in the shipped scripts (they capture fully
//!   built tables).  Upvalue→upvalue captures (`GETUPVAL` pseudo)
//!   share the cell exactly like Lua.
//! * **No metatables**, no coroutines, no `pcall` error objects,
//!   no string coercion on arithmetic, no `goto` (5.1 has none).
//! * Strings compare by content; tables/functions compare by
//!   identity — matching Lua value semantics.
//! * Number→string uses shortest round-trip formatting for
//!   non-integers (Lua 5.1 uses `%.14g`; identical for every value
//!   in the shipped scripts).
//! * `OP_CLOSE` is a no-op (cell upvalues never alias the stack).
//!
//! The VM is `#![forbid(unsafe_code)]` like the rest of the
//! workspace; recursion depth and instruction count are capped so
//! corrupt bytecode cannot hang or blow the native stack.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::{Instruction, Lua51File, LuaValue, Proto};
use prototype_common::{P3dError, Result};

/// Lua 5.1 `LFIELDS_PER_FLUSH` — PROVEN (see module docs).
pub const LFIELDS_PER_FLUSH: usize = 50;

/// Bias of the sBx operand (2^17 - 1).
const SBX_BIAS: i32 = 131_071;

// ---------------------------------------------------------------------------
// Runtime values
// ---------------------------------------------------------------------------

/// A runtime Lua value.
#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    /// Prototype's `lua_Number` is f32 (PROVEN).
    Num(f32),
    Str(Rc<[u8]>),
    Table(Rc<RefCell<Table>>),
    Closure(Rc<ClosureVal>),
    HostFn(Rc<HostCallable>),
}

impl Value {
    /// Lua truthiness: only `false` and `nil` are falsy.
    pub fn truth(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// Lua `type()` name.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Num(_) => "number",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Closure(_) | Value::HostFn(_) => "function",
        }
    }

    /// Human-readable form (test/debug output; `tostring` uses this).
    pub fn display(&self) -> String {
        match self {
            Value::Nil => "nil".into(),
            Value::Bool(b) => b.to_string(),
            Value::Num(n) => format_number(*n),
            Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
            Value::Table(t) => format!("table: 0x{:x}", Rc::as_ptr(t) as usize),
            Value::Closure(c) => format!("function: 0x{:x}", Rc::as_ptr(c) as usize),
            Value::HostFn(h) => format!("function: builtin '{}'", h.name),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}

/// Lua value equality: contents for primitives/strings, IDENTITY for
/// tables/functions (`t1 == t2` iff same object).
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b, // content compare
            (Value::Table(a), Value::Table(b)) => Rc::ptr_eq(a, b),
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::HostFn(a), Value::HostFn(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// Lua `%.14g`-compatible number formatting (see module docs for the
/// documented divergence on non-integers).
pub fn format_number(n: f32) -> String {
    if n.is_nan() {
        return "nan".into();
    }
    if n.is_infinite() {
        return if n.is_sign_negative() { "-inf".into() } else { "inf".into() };
    }
    if n == n.trunc() && n.abs() < 9.0e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Convert a parsed constant into a runtime value.
fn const_to_value(c: &LuaValue) -> Value {
    match c {
        LuaValue::Nil => Value::Nil,
        LuaValue::Bool(b) => Value::Bool(*b),
        LuaValue::Num(n) => Value::Num(*n),
        LuaValue::Str(s) => Value::Str(Rc::from(s.as_slice())),
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// A Lua table: insertion-ordered `(key, value)` entries.
///
/// Insertion order doubles as `next`/`pairs` iteration order, which
/// is exactly the order the bytecode built the table in — this makes
/// script runs deterministic and lets tests predict registration
/// order.  (Real Lua iterates its internal hash order; the *set* of
/// visited pairs is identical, and iteration order is unspecified
/// there too.)
#[derive(Debug, Default)]
pub struct Table {
    entries: Vec<(Value, Value)>,
}

impl Table {
    pub fn new() -> Self {
        Table { entries: Vec::new() }
    }

    /// Number of live entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate entries in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &(Value, Value)> {
        self.entries.iter()
    }

    /// Look up a key.  `None` means absent (callers turn that into
    /// `nil`); nil/NaN keys never match (Lua semantics).
    pub fn get(&self, key: &Value) -> Option<Value> {
        if matches!(key, Value::Nil) {
            return None;
        }
        if let Value::Num(n) = key {
            if n.is_nan() {
                return None;
            }
        }
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    /// Insert/replace; assigning nil REMOVES the key (Lua semantics).
    /// Returns an error for nil keys (`t[nil] = x` is a Lua error).
    pub fn set(&mut self, key: Value, val: Value) -> std::result::Result<(), P3dError> {
        if matches!(key, Value::Nil) {
            return Err(P3dError::Other(
                "table index is nil".into(),
            ));
        }
        if let Value::Num(n) = key {
            if n.is_nan() {
                return Err(P3dError::Other("table index is NaN".into()));
            }
        }
        if matches!(val, Value::Nil) {
            self.entries.retain(|(k, _)| *k != key);
            return Ok(());
        }
        for (k, v) in self.entries.iter_mut() {
            if *k == key {
                *v = val;
                return Ok(());
            }
        }
        self.entries.push((key, val));
        Ok(())
    }

    /// `next(t, k)`: the entry after `k` (`Nil` starts iteration).
    pub fn next(&self, key: &Value) -> Option<(Value, Value)> {
        if matches!(key, Value::Nil) {
            return self.entries.first().cloned();
        }
        let pos = self.entries.iter().position(|(k, _)| k == key)?;
        self.entries.get(pos + 1).cloned()
    }

    /// `#t` — border scan: largest `n` with `t[1..=n]` all present.
    pub fn array_len(&self) -> usize {
        let mut n = 0usize;
        while self
            .get(&Value::Num((n + 1) as f32))
            .is_some()
        {
            n += 1;
            if n > 1 << 24 {
                break; // defensive: corrupt tables cannot loop forever
            }
        }
        n
    }
}

// ---------------------------------------------------------------------------
// Closures and host functions
// ---------------------------------------------------------------------------

/// A Lua closure: a prototype (by arena index) plus captured upvalue
/// cells.
pub struct ClosureVal {
    /// Index into the VM's flattened proto arena.
    pub proto_id: usize,
    /// Upvalue cells (shared via `Rc`).
    pub upvals: Vec<Rc<RefCell<Value>>>,
}

impl fmt::Debug for ClosureVal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Closure(proto {}, {} upvals)", self.proto_id, self.upvals.len())
    }
}

/// A host (engine / library) function callable from Lua.
pub struct HostCallable {
    pub name: String,
    #[allow(clippy::type_complexity)]
    pub func: Box<dyn Fn(&mut Vm<'_>, &[Value]) -> Result<Vec<Value>>>,
}

impl fmt::Debug for HostCallable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HostFn({})", self.name)
    }
}

/// Automatic record of one host-function call (name + arguments).
#[derive(Debug, Clone)]
pub struct HostCall {
    pub name: String,
    pub args: Vec<Value>,
}

/// Host function type alias (for `register_host`).
pub type HostFn =
    Box<dyn Fn(&mut Vm<'_>, &[Value]) -> Result<Vec<Value>>>;

// ---------------------------------------------------------------------------
// The VM
// ---------------------------------------------------------------------------

/// The Lua 5.1 interpreter over a parsed chunk.
pub struct Vm<'a> {
    /// The global environment table (`_G`).
    pub globals: Rc<RefCell<Table>>,
    /// Flattened proto arena (pre-order; 0 = main chunk).
    protos: Vec<&'a Proto>,
    /// `children[i]` = arena ids of `protos[i].protos`.
    children: Vec<Vec<usize>>,
    /// Every host call, in order (engine natives AND library fns).
    pub host_calls: Vec<HostCall>,
    /// Instruction budget (anti-hang for corrupt bytecode).
    pub max_steps: usize,
    steps: usize,
    /// Call depth budget (anti-native-stack-overflow).
    pub max_depth: usize,
    depth: usize,
    /// Deterministic PRNG state for `math.random`.
    rng: u64,
}

fn flatten<'a>(p: &'a Proto, out: &mut Vec<&'a Proto>, kids: &mut Vec<Vec<usize>>) -> usize {
    let id = out.len();
    out.push(p);
    kids.push(Vec::new());
    for c in &p.protos {
        let cid = flatten(c, out, kids);
        kids[id].push(cid);
    }
    id
}

fn vm_err(msg: impl Into<String>) -> P3dError {
    P3dError::Other(msg.into())
}

impl<'a> Vm<'a> {
    /// Build a VM over a parsed chunk and install the base library
    /// (`pairs`, `next`, `math`, `string.format`, ...).
    pub fn new(file: &'a Lua51File) -> Result<Self> {
        let mut protos = Vec::new();
        let mut children = Vec::new();
        flatten(&file.main, &mut protos, &mut children);
        let mut vm = Vm {
            globals: Rc::new(RefCell::new(Table::new())),
            protos,
            children,
            host_calls: Vec::new(),
            max_steps: 5_000_000,
            steps: 0,
            max_depth: 200,
            depth: 0,
            rng: 0x9E3779B97F4A7C15,
        };
        vm.install_base_library()?;
        Ok(vm)
    }

    /// Register a host function as a global (engine natives typically
    /// record-and-return-nil stubs during analysis runs).
    pub fn register_host<F>(&mut self, name: &str, f: F) -> Result<()>
    where
        F: Fn(&mut Vm<'_>, &[Value]) -> Result<Vec<Value>> + 'static,
    {
        let v = Value::HostFn(Rc::new(HostCallable {
            name: name.to_string(),
            func: Box::new(f),
        }));
        self.set_global(name, v);
        Ok(())
    }

    /// Read a global (`nil` when absent).
    pub fn get_global(&self, name: &str) -> Value {
        self.globals
            .borrow()
            .get(&Value::Str(Rc::from(name.as_bytes())))
            .unwrap_or(Value::Nil)
    }

    /// Write a global.
    pub fn set_global(&mut self, name: &str, v: Value) {
        let key = Value::Str(Rc::from(name.as_bytes()));
        // Table::set only fails on nil keys; ours is a string.
        self.globals.borrow_mut().set(key, v).ok();
    }

    /// Call a function value with arguments (host calls get recorded).
    pub fn call_value(&mut self, f: &Value, args: Vec<Value>) -> Result<Vec<Value>> {
        match f {
            Value::Closure(cl) => self.exec_closure(cl, args),
            Value::HostFn(h) => {
                self.host_calls.push(HostCall {
                    name: h.name.clone(),
                    args: args.clone(),
                });
                (h.func)(self, &args)
            }
            other => Err(vm_err(format!(
                "attempt to call a {} value",
                other.type_name()
            ))),
        }
    }

    /// Call a global by name.
    pub fn call_global(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        let f = self.get_global(name);
        if matches!(f, Value::Nil) {
            return Err(vm_err(format!(
                "attempt to call global '{name}' (a nil value)"
            )));
        }
        self.call_value(&f, args.to_vec())
    }

    /// Execute the main chunk.
    pub fn run_main(&mut self) -> Result<Vec<Value>> {
        let cl = Rc::new(ClosureVal { proto_id: 0, upvals: Vec::new() });
        self.call_value(&Value::Closure(cl), Vec::new())
    }

    /// Recorded host calls with the given name, in call order.
    pub fn host_calls_of(&self, name: &str) -> Vec<Vec<Value>> {
        self.host_calls
            .iter()
            .filter(|c| c.name == name)
            .map(|c| c.args.clone())
            .collect()
    }

    /// Total instruction count executed so far.
    pub fn steps(&self) -> usize {
        self.steps
    }

    // -- base library ------------------------------------------------------

    /// The base globals provided by the VM (anything else a script
    /// reads must be a script-defined global or an engine native).
    pub const BASE_LIBRARY: &'static [&'static str] = &[
        "pairs", "next", "ipairs", "type", "tostring", "tonumber",
        "print", "unpack", "select", "assert", "math", "string",
    ];

    fn install_base_library(&mut self) -> Result<()> {
        self.register_host("pairs", |_vm, args| {
            match args.first() {
                Some(Value::Table(_)) => {
                    let next = _vm.get_global("next");
                    Ok(vec![next, args[0].clone(), Value::Nil])
                }
                _ => Err(vm_err(
                    "bad argument #1 to 'pairs' (table expected, got "
                        .to_string()
                        + &format!("{})", args.first().map(|a| a.type_name()).unwrap_or("no value")),
                )),
            }
        })?;

        self.register_host("next", |_vm, args| {
            let t = match args.first() {
                Some(Value::Table(t)) => t.clone(),
                _ => return Err(vm_err("bad argument #1 to 'next' (table expected)")),
            };
            let key = args.get(1).cloned().unwrap_or(Value::Nil);
            let out = t.borrow().next(&key);
            match out {
                Some((k, v)) => Ok(vec![k, v]),
                None => Ok(Vec::new()),
            }
        })?;

        self.register_host("ipairs", |_vm, args| {
            match args.first() {
                Some(Value::Table(t)) => {
                    let t = t.clone();
                    let iter = Value::HostFn(Rc::new(HostCallable {
                        name: "ipairs_iterator".into(),
                        func: Box::new(|_vm, args| {
                            let t = match args.first() {
                                Some(Value::Table(t)) => t.clone(),
                                _ => return Err(vm_err("ipairs iterator: table expected")),
                            };
                            let i = match args.get(1) {
                                Some(Value::Num(n)) => *n,
                                _ => return Err(vm_err("ipairs iterator: number expected")),
                            };
                            let k = i + 1.0;
                            let got = t.borrow().get(&Value::Num(k));
                            match got {
                                Some(v) => Ok(vec![Value::Num(k), v]),
                                None => Ok(Vec::new()),
                            }
                        }),
                    }));
                    Ok(vec![iter, Value::Table(t), Value::Num(0.0)])
                }
                _ => Err(vm_err("bad argument #1 to 'ipairs' (table expected)")),
            }
        })?;

        self.register_host("type", |_vm, args| {
            Ok(vec![Value::Str(Rc::from(
                args.first().map(|a| a.type_name()).unwrap_or("nil").as_bytes(),
            ))])
        })?;

        self.register_host("tostring", |_vm, args| {
            Ok(vec![Value::Str(Rc::from(
                args.first().map(|a| a.display()).unwrap_or_else(|| "nil".into()).as_bytes(),
            ))])
        })?;

        self.register_host("tonumber", |_vm, args| {
            let v = match args.first() {
                Some(v) => v.clone(),
                None => return Ok(Vec::new()),
            };
            match v {
                Value::Num(_) => Ok(vec![v]),
                Value::Str(s) => {
                    let t = String::from_utf8_lossy(&s);
                    let t = t.trim();
                    match t.parse::<f32>() {
                        Ok(n) => Ok(vec![Value::Num(n)]),
                        Err(_) => Ok(Vec::new()),
                    }
                }
                _ => Ok(Vec::new()),
            }
        })?;

        self.register_host("print", |_vm, args| {
            let line: Vec<String> = args.iter().map(|a| a.display()).collect();
            println!("{}", line.join("\t"));
            Ok(Vec::new())
        })?;

        self.register_host("unpack", |_vm, args| {
            let t = match args.first() {
                Some(Value::Table(t)) => t.clone(),
                _ => return Err(vm_err("bad argument #1 to 'unpack' (table expected)")),
            };
            let i = match args.get(1) {
                Some(Value::Num(n)) => *n as i64,
                _ => 1,
            };
            let j = match args.get(2) {
                Some(Value::Num(n)) => *n as i64,
                _ => t.borrow().array_len() as i64,
            };
            let mut out = Vec::new();
            for k in i..=j.max(i - 1) {
                out.push(
                    t.borrow()
                        .get(&Value::Num(k as f32))
                        .unwrap_or(Value::Nil),
                );
            }
            Ok(out)
        })?;

        self.register_host("select", |_vm, args| {
            match args.first() {
                Some(Value::Str(s)) if s.as_ref() == b"#" => {
                    Ok(vec![Value::Num((args.len() as f32) - 1.0)])
                }
                Some(Value::Num(n)) => {
                    let i = *n as usize;
                    if i < 1 || i >= args.len() {
                        Ok(Vec::new())
                    } else {
                        Ok(args[i..].to_vec())
                    }
                }
                _ => Err(vm_err("bad argument #1 to 'select'")),
            }
        })?;

        self.register_host("assert", |_vm, args| {
            if args.first().map(|a| a.truth()).unwrap_or(false) {
                Ok(args.to_vec())
            } else {
                Err(vm_err(
                    args.get(1)
                        .map(|m| m.display())
                        .unwrap_or_else(|| "assertion failed!".into()),
                ))
            }
        })?;

        // -- math table ----------------------------------------------------
        let math = Rc::new(RefCell::new(Table::new()));
        fn math1(name: &'static str, f: fn(f32) -> f32) -> Value {
            Value::HostFn(Rc::new(HostCallable {
                name: format!("math.{name}"),
                func: Box::new(move |_vm, args| {
                    match args.first() {
                        Some(Value::Num(n)) => Ok(vec![Value::Num(f(*n))]),
                        other => Err(vm_err(format!(
                            "bad argument #1 to 'math.{name}' (number expected, got {})",
                            other.map(|a| a.type_name()).unwrap_or("no value")
                        ))),
                    }
                }),
            }))
        }
        for (name, f) in [
            ("floor", f32::floor as fn(f32) -> f32),
            ("ceil", f32::ceil),
            ("abs", f32::abs),
            ("sqrt", f32::sqrt),
        ] {
            let key = Value::Str(Rc::from(name.as_bytes()));
            math.borrow_mut().set(key, math1(name, f)).ok();
        }
        for (name, _f) in [
            ("min", 0u8),
            ("max", 1u8),
        ] {
            let key = Value::Str(Rc::from(name.as_bytes()));
            let host = Value::HostFn(Rc::new(HostCallable {
                name: format!("math.{name}"),
                func: Box::new(move |_vm, args| {
                    let mut it = args.iter().filter_map(|a| match a {
                        Value::Num(n) => Some(*n),
                        _ => None,
                    });
                    let init = it.next().ok_or_else(|| {
                        vm_err(format!("bad argument #1 to 'math.{name}' (number expected)"))
                    })?;
                    let acc = it.fold(init, |acc, n| {
                        if (name == "min") == (n < acc) { n } else { acc }
                    });
                    Ok(vec![Value::Num(acc)])
                }),
            }));
            math.borrow_mut().set(key, host).ok();
        }
        {
            let key = Value::Str(Rc::from("pow".as_bytes()));
            math.borrow_mut()
                .set(
                    key,
                    Value::HostFn(Rc::new(HostCallable {
                        name: "math.pow".into(),
                        func: Box::new(|_vm, args| match (args.first(), args.get(1)) {
                            (Some(Value::Num(a)), Some(Value::Num(b))) => {
                                Ok(vec![Value::Num(a.powf(*b))])
                            }
                            _ => Err(vm_err("math.pow: number expected")),
                        }),
                    })),
                )
                .ok();
        }
        {
            let key = Value::Str(Rc::from("fmod".as_bytes()));
            // Lua 5.1 math.fmod: C fmodf (truncated).
            math.borrow_mut()
                .set(
                    key,
                    Value::HostFn(Rc::new(HostCallable {
                        name: "math.fmod".into(),
                        func: Box::new(|_vm, args| match (args.first(), args.get(1)) {
                            (Some(Value::Num(a)), Some(Value::Num(b))) => {
                                Ok(vec![Value::Num(a % b)])
                            }
                            _ => Err(vm_err("math.fmod: number expected")),
                        }),
                    })),
                )
                .ok();
        }
        {
            let key = Value::Str(Rc::from("random".as_bytes()));
            math.borrow_mut()
                .set(
                    key,
                    Value::HostFn(Rc::new(HostCallable {
                        name: "math.random".into(),
                        func: Box::new(|vm, args| {
                            let u = vm.next_rand();
                            match (args.first(), args.get(1)) {
                                (None, _) | (Some(Value::Nil), _) => {
                                    Ok(vec![Value::Num(u)])
                                }
                                (Some(Value::Num(m)), None) => {
                                    let m = m.floor();
                                    if m < 1.0 {
                                        return Err(vm_err(
                                            "bad argument #1 to 'math.random' (interval is empty)",
                                        ));
                                    }
                                    Ok(vec![Value::Num((u * m).floor() + 1.0)])
                                }
                                (Some(Value::Num(m)), Some(Value::Num(n))) => {
                                    let (m, n) = (m.floor(), n.floor());
                                    if n < m {
                                        return Err(vm_err(
                                            "bad argument #2 to 'math.random' (interval is empty)",
                                        ));
                                    }
                                    Ok(vec![Value::Num(
                                        m + (u * (n - m + 1.0)).floor(),
                                    )])
                                }
                                _ => Err(vm_err("math.random: number expected")),
                            }
                        }),
                    })),
                )
                .ok();
        }
        {
            let key = Value::Str(Rc::from("randomseed".as_bytes()));
            math.borrow_mut()
                .set(
                    key,
                    Value::HostFn(Rc::new(HostCallable {
                        name: "math.randomseed".into(),
                        func: Box::new(|vm, args| {
                            if let Some(Value::Num(n)) = args.first() {
                                vm.rng = n.to_bits() as u64 | 1;
                            }
                            Ok(Vec::new())
                        }),
                    })),
                )
                .ok();
        }
        math.borrow_mut()
            .set(Value::Str(Rc::from("pi".as_bytes())), Value::Num(std::f32::consts::PI))
            .ok();
        math.borrow_mut()
            .set(
                Value::Str(Rc::from("huge".as_bytes())),
                Value::Num(f32::INFINITY),
            )
            .ok();
        self.set_global(
            "math",
            Value::Table(math),
        );

        // -- string table (format only) ------------------------------------
        let string = Rc::new(RefCell::new(Table::new()));
        {
            let key = Value::Str(Rc::from("format".as_bytes()));
            string
                .borrow_mut()
                .set(
                    key,
                    Value::HostFn(Rc::new(HostCallable {
                        name: "string.format".into(),
                        func: Box::new(|_vm, args| {
                            let f = match args.first() {
                                Some(Value::Str(s)) => s.clone(),
                                _ => return Err(vm_err("string.format: string expected")),
                            };
                            let mut out: Vec<u8> = Vec::new();
                            let mut ai = 1usize;
                            let spec = String::from_utf8_lossy(&f).into_owned();
                            let mut chars = spec.chars().peekable();
                            while let Some(c) = chars.next() {
                                if c != '%' {
                                    out.extend_from_slice(c.to_string().as_bytes());
                                    continue;
                                }
                                let mut spec_chars = String::new();
                                // flags/width/precision
                                while let Some(&n) = chars.peek() {
                                    if n.is_ascii_digit() || n == '.' || n == '-' || n == '+' {
                                        spec_chars.push(n);
                                        chars.next();
                                    } else {
                                        break;
                                    }
                                }
                                let conv = chars.next().ok_or_else(|| {
                                    vm_err("string.format: invalid conversion '%'")
                                })?;
                                match conv {
                                    '%' => out.push(b'%'),
                                    'd' | 'i' => {
                                        let n = match args.get(ai) {
                                            Some(Value::Num(n)) => *n as i64,
                                            _ => return Err(vm_err(
                                                "string.format: number expected",
                                            )),
                                        };
                                        ai += 1;
                                        out.extend_from_slice(n.to_string().as_bytes());
                                    }
                                    'f' | 'g' | 'G' | 'e' | 'E' => {
                                        let n = match args.get(ai) {
                                            Some(Value::Num(n)) => *n,
                                            _ => return Err(vm_err(
                                                "string.format: number expected",
                                            )),
                                        };
                                        ai += 1;
                                        out.extend_from_slice(
                                            format_number(n).as_bytes(),
                                        );
                                    }
                                    's' => {
                                        let s = match args.get(ai) {
                                            Some(v) => v.display(),
                                            None => String::new(),
                                        };
                                        ai += 1;
                                        out.extend_from_slice(s.as_bytes());
                                    }
                                    other => {
                                        return Err(vm_err(format!(
                                            "string.format: unsupported conversion '%{other}'"
                                        )))
                                    }
                                }
                            }
                            Ok(vec![Value::Str(Rc::from(out))])
                        }),
                    })),
                )
                .ok();
        }
        self.set_global("string", Value::Table(string));

        Ok(())
    }

    /// Deterministic xorshift64 → [0,1) for `math.random`.
    fn next_rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        ((x >> 40) as f32) / ((1u64 << 24) as f32)
    }

    // -- interpreter -------------------------------------------------------

    fn exec_closure(
        &mut self,
        cl: &Rc<ClosureVal>,
        args: Vec<Value>,
    ) -> Result<Vec<Value>> {
        if self.depth >= self.max_depth {
            return Err(vm_err(format!(
                "stack overflow (depth > {}); likely runaway recursion",
                self.max_depth
            )));
        }
        self.depth += 1;
        let out = self.exec_inner(cl, args);
        self.depth -= 1;
        out
    }

    /// The interpreter loop (one register frame).
    fn exec_inner(
        &mut self,
        cl: &Rc<ClosureVal>,
        args: Vec<Value>,
    ) -> Result<Vec<Value>> {
        let proto = self.protos[cl.proto_id];
        let nparams = proto.nparams as usize;
        let is_vararg = proto.is_vararg != 0;
        let stack_len = (proto.maxstacksize as usize).max(1);
        // Copy code + constants out of the borrow graph so `&mut self`
        // stays usable for nested calls (see module docs: simplicity
        // over allocation).
        let code: Vec<u32> = proto.code.clone();
        let consts: Vec<Value> =
            proto.constants.iter().map(const_to_value).collect();
        let upvals: Vec<Rc<RefCell<Value>>> = cl.upvals.clone();
        let frame_proto_id = cl.proto_id;

        let mut regs: Vec<Value> = vec![Value::Nil; stack_len];
        let nfixed = nparams.min(args.len());
        for i in 0..nfixed {
            regs[i] = args[i].clone();
        }
        let varargs: Vec<Value> = if is_vararg {
            args[nfixed..].to_vec()
        } else {
            Vec::new()
        };

        let base_top = stack_len;
        let mut top = base_top;
        let mut pc: usize = 0;

        // RK access: operand >= 256 → constant, else register.
        macro_rules! rk {
            ($x:expr) => {
                if ($x as usize) >= 256 {
                    consts
                        .get($x as usize - 256)
                        .cloned()
                        .unwrap_or(Value::Nil)
                } else {
                    regs.get($x as usize).cloned().unwrap_or(Value::Nil)
                }
            };
        }
        macro_rules! reg_err {
            ($idx:expr, $op:expr) => {
                vm_err(format!(
                    "register R{} out of range in {} (corrupt bytecode?)",
                    $idx, $op
                ))
            };
        }
        // Grow-on-write register access (corrupt files must error, not panic).
        macro_rules! setreg {
            ($idx:expr, $v:expr) => {{
                let i = ($idx) as usize;
                while regs.len() <= i {
                    regs.push(Value::Nil);
                }
                regs[i] = $v;
            }};
        }
        // Read a register as a value (Nil when out of frame range).
        macro_rules! getreg {
            ($idx:expr) => {
                regs.get(($idx) as usize).cloned().unwrap_or(Value::Nil)
            };
        }

        loop {
            if pc >= code.len() {
                // Fell off the end without RETURN — tolerate as no results.
                return Ok(Vec::new());
            }
            self.steps += 1;
            if self.steps > self.max_steps {
                return Err(vm_err(format!(
                    "instruction budget exceeded ({} steps) — possible infinite loop",
                    self.max_steps
                )));
            }
            let ins = Instruction::decode(code[pc]);
            pc += 1;

            match ins.opcode {
                // MOVE A B: R[A] := R[B]
                0 => {
                    let v = regs
                        .get(ins.b as usize)
                        .cloned()
                        .ok_or_else(|| reg_err!(ins.b, "MOVE"))?;
                    let a = ins.a as usize;
                    while regs.len() <= a {
                        regs.push(Value::Nil);
                    }
                    regs[a] = v;
                    top = base_top;
                }
                // LOADK A Bx: R[A] := K[Bx]
                1 => {
                    let v = consts
                        .get(ins.bx as usize)
                        .cloned()
                        .unwrap_or(Value::Nil);
                    let a = ins.a as usize;
                    while regs.len() <= a {
                        regs.push(Value::Nil);
                    }
                    regs[a] = v;
                    top = base_top;
                }
                // LOADBOOL A B C
                2 => {
                    setreg!(ins.a, Value::Bool(ins.b != 0));
                    if ins.c != 0 {
                        pc += 1;
                    }
                    top = base_top;
                }
                // LOADNIL A B: R[A..A+B] := nil
                3 => {
                    for i in ins.a as usize..=(ins.a + ins.b) as usize {
                        while regs.len() <= i {
                            regs.push(Value::Nil);
                        }
                        regs[i] = Value::Nil;
                    }
                    top = base_top;
                }
                // GETUPVAL A B
                4 => {
                    let cell = upvals
                        .get(ins.b as usize)
                        .ok_or_else(|| reg_err!(ins.b, "GETUPVAL"))?;
                    let v = cell.borrow().clone();
                    setreg!(ins.a, v);
                    top = base_top;
                }
                // GETGLOBAL A Bx: R[A] := Gbl[K[Bx]]
                5 => {
                    let key = consts
                        .get(ins.bx as usize)
                        .cloned()
                        .unwrap_or(Value::Nil);
                    let name = match &key {
                        Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
                        _ => "<?>".to_string(),
                    };
                    let v = self.get_global(&name);
                    setreg!(ins.a, v);
                    top = base_top;
                }
                // GETTABLE A B C: R[A] := R[B][RK[C]]
                6 => {
                    let t = regs
                        .get(ins.b as usize)
                        .cloned()
                        .ok_or_else(|| reg_err!(ins.b, "GETTABLE"))?;
                    let k = rk!(ins.c);
                    let v = index_get(&t, &k)?;
                    setreg!(ins.a, v);
                    top = base_top;
                }
                // SETGLOBAL A Bx
                7 => {
                    let key = consts
                        .get(ins.bx as usize)
                        .cloned()
                        .unwrap_or(Value::Nil);
                    let name = match &key {
                        Value::Str(s) => String::from_utf8_lossy(s).into_owned(),
                        _ => {
                            return Err(vm_err("SETGLOBAL with non-string constant"))
                        }
                    };
                    let v = getreg!(ins.a);
                    self.set_global(&name, v);
                    top = base_top;
                }
                // SETUPVAL A B
                8 => {
                    let v = getreg!(ins.a);
                    let cell = upvals
                        .get(ins.b as usize)
                        .ok_or_else(|| reg_err!(ins.b, "SETUPVAL"))?;
                    *cell.borrow_mut() = v;
                    top = base_top;
                }
                // SETTABLE A B C: R[A][RK[B]] := RK[C]
                9 => {
                    let t = getreg!(ins.a);
                    let k = rk!(ins.b);
                    let v = rk!(ins.c);
                    index_set(&t, k, v)?;
                    top = base_top;
                }
                // NEWTABLE A B C (size hints ignored)
                10 => {
                    setreg!(
                        ins.a,
                        Value::Table(Rc::new(RefCell::new(Table::new())))
                    );
                    top = base_top;
                }
                // SELF A B C: R[A+1] := R[B]; R[A] := R[B][RK[C]]
                11 => {
                    let o = regs
                        .get(ins.b as usize)
                        .cloned()
                        .ok_or_else(|| reg_err!(ins.b, "SELF"))?;
                    let k = rk!(ins.c);
                    let f = index_get(&o, &k)?;
                    let a = ins.a as usize;
                    while regs.len() <= a + 1 {
                        regs.push(Value::Nil);
                    }
                    regs[a + 1] = o;
                    regs[a] = f;
                    top = base_top;
                }
                // ADD..POW A B C: R[A] := RK[B] op RK[C]
                12..=17 => {
                    let x = rk!(ins.b);
                    let y = rk!(ins.c);
                    let v = arith(ins.opcode, &x, &y)?;
                    setreg!(ins.a, v);
                    top = base_top;
                }
                // UNM A B
                18 => {
                    let x = regs
                        .get(ins.b as usize)
                        .cloned()
                        .ok_or_else(|| reg_err!(ins.b, "UNM"))?;
                    setreg!(ins.a, match x {
                        Value::Num(n) => Value::Num(-n),
                        other => {
                            return Err(vm_err(format!(
                                "attempt to perform arithmetic on a {} value",
                                other.type_name()
                            )))
                        }
                    });
                    top = base_top;
                }
                // NOT A B
                19 => {
                    let b = regs
                        .get(ins.b as usize)
                        .cloned()
                        .unwrap_or(Value::Nil);
                    setreg!(ins.a, Value::Bool(!b.truth()));
                    top = base_top;
                }
                // LEN A B: R[A] := #R[B]
                20 => {
                    let v = regs
                        .get(ins.b as usize)
                        .cloned()
                        .ok_or_else(|| reg_err!(ins.b, "LEN"))?;
                    setreg!(
                        ins.a,
                        match v {
                            Value::Str(s) => Value::Num(s.len() as f32),
                            Value::Table(t) => {
                                Value::Num(t.borrow().array_len() as f32)
                            }
                            other => {
                                return Err(vm_err(format!(
                                    "attempt to get length of a {} value",
                                    other.type_name()
                                )))
                            }
                        }
                    );
                    top = base_top;
                }
                // CONCAT A B C: R[A] := R[B] .. .. R[C]
                21 => {
                    let mut buf: Vec<u8> = Vec::new();
                    for i in ins.b as usize..=ins.c as usize {
                        let v = regs
                            .get(i)
                            .cloned()
                            .ok_or_else(|| reg_err!(i, "CONCAT"))?;
                        match v {
                            Value::Str(s) => buf.extend_from_slice(&s),
                            Value::Num(n) => {
                                buf.extend_from_slice(format_number(n).as_bytes())
                            }
                            other => {
                                return Err(vm_err(format!(
                                    "attempt to concatenate a {} value",
                                    other.type_name()
                                )))
                            }
                        }
                    }
                    setreg!(ins.a, Value::Str(Rc::from(buf)));
                    top = base_top;
                }
                // JMP sBx
                22 => {
                    pc = (pc as i64 + ins.sbx as i64) as usize;
                    top = base_top;
                }
                // EQ/LT/LE A B C: if (R[A] op RK[B]) ~= C then pc++
                23..=25 => {
                    let x = getreg!(ins.a);
                    let y = rk!(ins.b);
                    let cmp = match ins.opcode {
                        23 => x == y,
                        24 => compare_lt(&x, &y)?,
                        _ => compare_le(&x, &y)?,
                    };
                    if cmp != (ins.c != 0) {
                        pc += 1;
                    }
                    top = base_top;
                }
                // TEST A C: if not (truth(R[A]) == C) then pc++
                26 => {
                    let t = getreg!(ins.a).truth();
                    if t != (ins.c != 0) {
                        pc += 1;
                    }
                    top = base_top;
                }
                // TESTSET A B C: if truth(R[B]) == C then R[A] := R[B] else pc++
                27 => {
                    let b = regs
                        .get(ins.b as usize)
                        .cloned()
                        .ok_or_else(|| reg_err!(ins.b, "TESTSET"))?;
                    if b.truth() == (ins.c != 0) {
                        setreg!(ins.a, b);
                    } else {
                        pc += 1;
                    }
                    top = base_top;
                }
                // CALL A B C
                28 => {
                    let a = ins.a as usize;
                    let nargs = if ins.b == 0 {
                        top.saturating_sub(a + 1)
                    } else {
                        ins.b as usize - 1
                    };
                    while regs.len() < a + 1 + nargs {
                        regs.push(Value::Nil);
                    }
                    let call_args: Vec<Value> =
                        regs[a + 1..a + 1 + nargs].to_vec();
                    let callee = regs[a].clone();
                    let results = self.call_value(&callee, call_args)?;
                    if ins.c == 0 {
                        // keep ALL results; raise top
                        let n = results.len();
                        while regs.len() < a + n {
                            regs.push(Value::Nil);
                        }
                        for (i, r) in results.into_iter().enumerate() {
                            regs[a + i] = r;
                        }
                        top = a + n;
                    } else {
                        let want = ins.c as usize - 1;
                        for i in 0..want {
                            while regs.len() <= a + i {
                                regs.push(Value::Nil);
                            }
                            regs[a + i] =
                                results.get(i).cloned().unwrap_or(Value::Nil);
                        }
                        top = base_top;
                    }
                }
                // TAILCALL A B: return f(args) to the caller
                29 => {
                    let a = ins.a as usize;
                    let nargs = if ins.b == 0 {
                        top.saturating_sub(a + 1)
                    } else {
                        ins.b as usize - 1
                    };
                    while regs.len() < a + 1 + nargs {
                        regs.push(Value::Nil);
                    }
                    let call_args: Vec<Value> =
                        regs[a + 1..a + 1 + nargs].to_vec();
                    let callee = regs[a].clone();
                    return self.call_value(&callee, call_args);
                }
                // RETURN A B
                30 => {
                    let a = ins.a as usize;
                    let vals = if ins.b == 0 {
                        let end = top.max(a);
                        while regs.len() < end {
                            regs.push(Value::Nil);
                        }
                        regs[a..end].to_vec()
                    } else {
                        let end = a + ins.b as usize - 1;
                        while regs.len() < end {
                            regs.push(Value::Nil);
                        }
                        regs[a..end].to_vec()
                    };
                    return Ok(vals);
                }
                // FORLOOP A sBx
                31 => {
                    let a = ins.a as usize;
                    let step = as_num(&getreg!(a + 2), "'for' step")?;
                    let idx = as_num(&getreg!(a), "'for' index")? + step;
                    let limit = as_num(&getreg!(a + 1), "'for' limit")?;
                    if (step > 0.0 && idx <= limit) || (step < 0.0 && idx >= limit) {
                        setreg!(a, Value::Num(idx));
                        setreg!(a + 3, Value::Num(idx));
                        pc = (pc as i64 + ins.sbx as i64) as usize;
                    }
                    top = base_top;
                }
                // FORPREP A sBx
                32 => {
                    let a = ins.a as usize;
                    let init = as_num(&getreg!(a), "'for' initial value")?;
                    let _limit = as_num(&getreg!(a + 1), "'for' limit")?;
                    let step = as_num(&getreg!(a + 2), "'for' step")?;
                    if step == 0.0 {
                        return Err(vm_err("'for' step is zero"));
                    }
                    setreg!(a, Value::Num(init - step));
                    pc = (pc as i64 + ins.sbx as i64) as usize;
                    top = base_top;
                }
                // TFORLOOP A C
                33 => {
                    let a = ins.a as usize;
                    let iter = getreg!(a);
                    let state = getreg!(a + 1);
                    let control = getreg!(a + 2);
                    let c = ins.c as usize;
                    let results =
                        self.call_value(&iter, vec![state, control])?;
                    while regs.len() < a + 3 + c {
                        regs.push(Value::Nil);
                    }
                    for i in 0..c {
                        regs[a + 3 + i] =
                            results.get(i).cloned().unwrap_or(Value::Nil);
                    }
                    if !matches!(regs[a + 3], Value::Nil) {
                        regs[a + 2] = regs[a + 3].clone();
                        // fall through: the following JMP runs (to body)
                    } else {
                        pc += 1; // skip the JMP — loop finished
                    }
                    top = base_top;
                }
                // SETLIST A B C
                34 => {
                    let a = ins.a as usize;
                    let mut n = ins.b as usize;
                    if n == 0 {
                        n = top.saturating_sub(a + 1);
                    }
                    let mut c = ins.c as usize;
                    if c == 0 {
                        if pc >= code.len() {
                            return Err(vm_err("SETLIST: missing C extra word"));
                        }
                        c = code[pc] as usize;
                        pc += 1;
                    }
                    let t = match regs.get(a) {
                        Some(Value::Table(t)) => t.clone(),
                        other => {
                            return Err(vm_err(format!(
                                "SETLIST on a {} value",
                                other.map(|o| o.type_name()).unwrap_or("nil")
                            )))
                        }
                    };
                    let start = (c - 1) * LFIELDS_PER_FLUSH;
                    for j in 1..=n {
                        let v = regs
                            .get(a + j)
                            .cloned()
                            .ok_or_else(|| reg_err!(a + j, "SETLIST"))?;
                        t.borrow_mut()
                            .set(Value::Num((start + j) as f32), v)?;
                    }
                    top = base_top;
                }
                // CLOSE A — no-op under cell upvalues (module docs)
                35 => {
                    top = base_top;
                }
                // CLOSURE A Bx (+ nups pseudo-instructions, where nups
                // is the CHILD proto's upvalue count)
                36 => {
                    let bx = ins.bx as usize;
                    let child = *self
                        .children
                        .get(frame_proto_id)
                        .and_then(|kids| kids.get(bx))
                        .ok_or_else(|| {
                            vm_err(format!(
                                "CLOSURE: child proto #{bx} of proto {frame_proto_id} not found"
                            ))
                        })?;
                    // PROVEN (ldump.c/lvm.c): the pseudo-instruction count
                    // is the CLOSED proto's nups, not the current frame's.
                    let child_nups = self.protos[child].nups as usize;
                    let mut ups: Vec<Rc<RefCell<Value>>> =
                        Vec::with_capacity(child_nups);
                    for _ in 0..child_nups {
                        if pc >= code.len() {
                            return Err(vm_err(
                                "CLOSURE: missing upvalue pseudo-instruction",
                            ));
                        }
                        let pseudo = Instruction::decode(code[pc]);
                        pc += 1;
                        match pseudo.opcode {
                            // MOVE A B: upvalue A captures stack slot B
                            0 => {
                                let v = regs
                                    .get(pseudo.b as usize)
                                    .cloned()
                                    .unwrap_or(Value::Nil);
                                ups.push(Rc::new(RefCell::new(v)));
                            }
                            // GETUPVAL A B: upvalue A shares parent's upvalue B
                            4 => {
                                let cell = upvals
                                    .get(pseudo.b as usize)
                                    .ok_or_else(|| {
                                        reg_err!(pseudo.b, "CLOSURE/GETUPVAL")
                                    })?
                                    .clone();
                                ups.push(cell);
                            }
                            other => {
                                return Err(vm_err(format!(
                                    "CLOSURE: bad upvalue pseudo-op {other}"
                                )))
                            }
                        }
                    }
                    setreg!(
                        ins.a,
                        Value::Closure(Rc::new(ClosureVal {
                            proto_id: child,
                            upvals: ups,
                        }))
                    );
                    top = base_top;
                }
                // VARARG A B
                37 => {
                    let a = ins.a as usize;
                    if ins.b == 0 {
                        while regs.len() < a + varargs.len() {
                            regs.push(Value::Nil);
                        }
                        for (i, v) in varargs.iter().cloned().enumerate() {
                            regs[a + i] = v;
                        }
                        top = a + varargs.len();
                    } else {
                        let n = ins.b as usize - 1;
                        for i in 0..n {
                            while regs.len() <= a + i {
                                regs.push(Value::Nil);
                            }
                            regs[a + i] =
                                varargs.get(i).cloned().unwrap_or(Value::Nil);
                        }
                        top = base_top;
                    }
                }
                other => {
                    return Err(vm_err(format!(
                        "unimplemented opcode {other} ({})",
                        ins.opname()
                    )))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Operand helpers
// ---------------------------------------------------------------------------

fn as_num(v: &Value, what: &str) -> Result<f32> {
    match v {
        Value::Num(n) => Ok(*n),
        other => Err(vm_err(format!(
            "{what} must be a number (got {})",
            other.type_name()
        ))),
    }
}

/// OP_ADD(12)..OP_POW(17).
fn arith(op: u8, x: &Value, y: &Value) -> Result<Value> {
    let (a, b) = match (x, y) {
        (Value::Num(a), Value::Num(b)) => (*a, *b),
        _ => {
            return Err(vm_err(format!(
                "attempt to perform arithmetic on a {} value",
                if !matches!(x, Value::Num(_)) {
                    x.type_name()
                } else {
                    y.type_name()
                }
            )))
        }
    };
    Ok(Value::Num(match op {
        12 => a + b,
        13 => a - b,
        14 => a * b,
        15 => a / b,
        // Lua 5.1 luai_nummod: a - floor(a/b)*b (floor modulo)
        16 => a - (a / b).floor() * b,
        17 => a.powf(b),
        _ => unreachable!("arith opcode range checked by caller"),
    }))
}

fn compare_lt(x: &Value, y: &Value) -> Result<bool> {
    match (x, y) {
        (Value::Num(a), Value::Num(b)) => Ok(a < b),
        (Value::Str(a), Value::Str(b)) => Ok(**a < **b),
        _ => Err(vm_err(format!(
            "attempt to compare {} with {}",
            x.type_name(),
            y.type_name()
        ))),
    }
}

fn compare_le(x: &Value, y: &Value) -> Result<bool> {
    match (x, y) {
        (Value::Num(a), Value::Num(b)) => Ok(a <= b),
        (Value::Str(a), Value::Str(b)) => Ok(**a <= **b),
        _ => Err(vm_err(format!(
            "attempt to compare {} with {}",
            x.type_name(),
            y.type_name()
        ))),
    }
}

fn index_get(t: &Value, k: &Value) -> Result<Value> {
    match t {
        Value::Table(tb) => Ok(tb.borrow().get(k).unwrap_or(Value::Nil)),
        other => Err(vm_err(format!(
            "attempt to index a {} value",
            other.type_name()
        ))),
    }
}

fn index_set(t: &Value, k: Value, v: Value) -> Result<()> {
    match t {
        Value::Table(tb) => tb.borrow_mut().set(k, v),
        other => Err(vm_err(format!(
            "attempt to index a {} value",
            other.type_name()
        ))),
    }
}

// keep SBX_BIAS referenced (documents the decode contract)
#[allow(dead_code)]
const _: () = {
    assert!(SBX_BIAS == 131_071);
};

#[cfg(test)]
#[path = "vm_tests.rs"]
mod tests;
