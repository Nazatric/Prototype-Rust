//! vm tests — synthetic bytecode (via a small assembler mirroring the
//! PROVEN decode layout) plus the real-fixture acceptance runs.

use super::*;
use crate::{Lua51File, Lua51Header, LuaValue, Proto};
use std::path::Path;

// -- fixtures --------------------------------------------------------------

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("prototype-bin")
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("missing fixture {p:?}: {e}"))
}

// -- tiny assembler (mirrors Instruction::decode — PROVEN layout) -----------

const SBX_BIAS: i32 = 131_071;

fn abc(op: u32, a: u32, b: u32, c: u32) -> u32 {
    op | (a << 6) | (c << 14) | (b << 23)
}
fn abx(op: u32, a: u32, bx: u32) -> u32 {
    op | (a << 6) | (bx << 14)
}
fn asbx(op: u32, a: u32, sbx: i32) -> u32 {
    abx(op, a, (sbx + SBX_BIAS) as u32)
}

/// OP codes used by the assembler below.
#[allow(dead_code)]
mod op {
    pub const MOVE: u32 = 0;
    pub const LOADK: u32 = 1;
    pub const LOADBOOL: u32 = 2;
    pub const LOADNIL: u32 = 3;
    pub const GETUPVAL: u32 = 4;
    pub const GETGLOBAL: u32 = 5;
    pub const GETTABLE: u32 = 6;
    pub const SETGLOBAL: u32 = 7;
    pub const SETUPVAL: u32 = 8;
    pub const SETTABLE: u32 = 9;
    pub const NEWTABLE: u32 = 10;
    pub const SELF: u32 = 11;
    pub const ADD: u32 = 12;
    pub const SUB: u32 = 13;
    pub const MUL: u32 = 14;
    pub const DIV: u32 = 15;
    pub const MOD: u32 = 16;
    pub const POW: u32 = 17;
    pub const UNM: u32 = 18;
    pub const NOT: u32 = 19;
    pub const LEN: u32 = 20;
    pub const CONCAT: u32 = 21;
    pub const JMP: u32 = 22;
    pub const EQ: u32 = 23;
    pub const LT: u32 = 24;
    pub const LE: u32 = 25;
    pub const TEST: u32 = 26;
    pub const TESTSET: u32 = 27;
    pub const CALL: u32 = 28;
    pub const TAILCALL: u32 = 29;
    pub const RETURN: u32 = 30;
    pub const FORLOOP: u32 = 31;
    pub const FORPREP: u32 = 32;
    pub const TFORLOOP: u32 = 33;
    pub const SETLIST: u32 = 34;
    pub const CLOSE: u32 = 35;
    pub const CLOSURE: u32 = 36;
    pub const VARARG: u32 = 37;
}

fn header() -> Lua51Header {
    Lua51Header {
        magic: [0x1b, b'L', b'u', b'a'],
        version: 0x51,
        format: 0,
        endian: 1,
        sizeof_int: 4,
        sizeof_size_t: 4,
        sizeof_instruction: 4,
        sizeof_lua_number: 4,
        integral_flag: 0,
    }
}

fn proto(
    code: Vec<u32>,
    constants: Vec<LuaValue>,
    protos: Vec<Proto>,
    nparams: u8,
    is_vararg: bool,
) -> Proto {
    Proto {
        source: None,
        linedefined: 0,
        lastlinedefined: 0,
        nups: 0,
        nparams,
        is_vararg: if is_vararg { 2 } else { 0 },
        maxstacksize: 40, // generous; the VM grows as needed
        code,
        constants,
        protos,
        lineinfo: Vec::new(),
        locvars: Vec::new(),
        upvalues: Vec::new(),
    }
}

fn file_of(main: Proto) -> Lua51File {
    Lua51File { header: header(), main, trailing: Vec::new() }
}

fn s(v: &str) -> LuaValue {
    LuaValue::Str(v.as_bytes().to_vec())
}
fn n(v: f32) -> LuaValue {
    LuaValue::Num(v)
}

fn stub_hosts(vm: &mut Vm<'_>, names: &[&str]) {
    for name in names {
        vm.register_host(name, |_vm, _args| Ok(Vec::new())).unwrap();
    }
}

// ===========================================================================
// Synthetic opcode tests
// ===========================================================================

#[test]
fn vm_arith_ops_compute() {
    // R0 = 7, R1 = 2; exercise every binary op and UNM; return table.
    let code = vec![
        abx(op::LOADK, 0, 0),        // R0 = K0 = 7
        abx(op::LOADK, 1, 1),        // R1 = K1 = 2
        abc(op::ADD, 2, 0, 1),       // R2 = 9
        abc(op::SUB, 3, 0, 1),       // R3 = 5
        abc(op::MUL, 4, 0, 1),       // R4 = 14
        abc(op::DIV, 5, 0, 1),       // R5 = 3.5
        abc(op::MOD, 6, 0, 1),       // R6 = 1
        abc(op::POW, 7, 0, 1),       // R7 = 49
        abc(op::UNM, 8, 0, 0),       // R8 = -7
        abc(op::RETURN, 2, 8, 0),    // return R2..R8 (7 values: B-1)
    ];
    let main = proto(code, vec![n(7.0), n(2.0)], vec![], 0, true);
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    let nums: Vec<f32> = out
        .iter()
        .map(|v| match v {
            Value::Num(x) => *x,
            other => panic!("expected number, got {}", other.display()),
        })
        .collect();
    assert_eq!(nums, vec![9.0, 5.0, 14.0, 3.5, 1.0, 49.0, -7.0]);
}

#[test]
fn vm_numeric_for_loop_sums_1_to_10() {
    // acc = 0; for i = 1, 10, 1 do acc = acc + i end; return acc
    // Lua layout: FORPREP jumps to FORLOOP; the body falls THROUGH
    // to FORLOOP; FORLOOP jumps back to the body while in range
    // (PROVEN shape — mirrors lvm.c control flow).  The control
    // variable is R[A+3] — keep the accumulator OUT of R0..R3.
    let code = vec![
        abx(op::LOADK, 0, 0),      // 0: R0 = 1        (init)
        abx(op::LOADK, 1, 1),      // 1: R1 = 10       (limit)
        abx(op::LOADK, 2, 2),      // 2: R2 = 1        (step)
        abx(op::LOADK, 4, 3),      // 3: R4 = 0        (acc)
        asbx(op::FORPREP, 0, 1),   // 4: -> 6 (FORLOOP)
        abc(op::ADD, 4, 4, 3),     // 5: acc += i (control var R3)
        asbx(op::FORLOOP, 0, -2),  // 6: -> 5 while in range
        abc(op::RETURN, 4, 2, 0),  // 7: return acc
    ];
    let main = proto(code, vec![n(1.0), n(10.0), n(1.0), n(0.0)], vec![], 0, true);
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out, vec![Value::Num(55.0)]);
}

#[test]
fn vm_for_loop_with_float_step_counts_down() {
    // for i = 2, 0, -0.5: count iterations (control var at R3)
    let code = vec![
        abx(op::LOADK, 0, 0),       // R0 = 2
        abx(op::LOADK, 1, 1),       // R1 = 0
        abx(op::LOADK, 2, 2),       // R2 = -0.5
        abx(op::LOADK, 4, 3),       // R4 = 0 (count)
        asbx(op::FORPREP, 0, 1),    // 4: -> 6 (FORLOOP)
        abc(op::ADD, 4, 4, 256 + 4), // 5: R4 += K4(1)
        asbx(op::FORLOOP, 0, -2),   // 6: -> 5
        abc(op::RETURN, 4, 2, 0),   // 7
    ];
    let main = proto(
        code,
        vec![n(2.0), n(0.0), n(-0.5), n(0.0), n(1.0)],
        vec![],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    // 2, 1.5, 1, 0.5, 0 → 5 iterations
    assert_eq!(out, vec![Value::Num(5.0)]);
}

#[test]
fn vm_setlist_len_gettable() {
    // t = {10, 20, 30}; return #t, t[2], t["k"] (k set separately)
    let code = vec![
        abc(op::NEWTABLE, 0, 0, 0),      // 0: R0 = {}
        abx(op::LOADK, 1, 0),            // 1: R1 = 10
        abx(op::LOADK, 2, 1),            // 2: R2 = 20
        abx(op::LOADK, 3, 2),            // 3: R3 = 30
        abc(op::SETLIST, 0, 3, 1),       // 4: R0[1..3] = R1..R3
        abc(op::SETTABLE, 0, 256 + 3, 256 + 4), // 5: R0["k"] = 99
        abc(op::LEN, 1, 0, 0),           // 6: R1 = #R0
        abc(op::GETTABLE, 2, 0, 1),      // 7: R2 = R0[R1] (= t[3] = 30)
        abc(op::GETTABLE, 3, 0, 256 + 3), // 8: R3 = R0["k"] = 99
        abc(op::RETURN, 1, 4, 0),        // 9: return R1, R2, R3
    ];
    let main = proto(
        code,
        vec![n(10.0), n(20.0), n(30.0), s("k"), n(99.0)],
        vec![],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(
        out,
        vec![Value::Num(3.0), Value::Num(30.0), Value::Num(99.0)]
    );
}

#[test]
fn vm_closures_mutate_shared_upvalue() {
    // main: local x = 100; bump = function() x = x + 1; return x end
    //       bump(); bump(); return bump()
    // child: GETUPVAL R0 0; ADD R0 R0 K(1); SETUPVAL 0 0; RETURN R0 2
    let child = proto(
        vec![
            abc(op::GETUPVAL, 0, 0, 0),
            abc(op::ADD, 0, 0, 256), // R0 = R0 + K0(1)
            abc(op::SETUPVAL, 0, 0, 0),
            abc(op::RETURN, 0, 2, 0),
        ],
        vec![n(1.0)],
        vec![],
        0,
        false,
    );
    let mut main = proto(
        vec![
            abx(op::LOADK, 0, 0),     // 0: R0 = 100
            abx(op::CLOSURE, 1, 0),   // 1: R1 = closure(child)
            abc(op::MOVE, 0, 0, 0),   // 2: pseudo — upval 0 <- R0
            abx(op::SETGLOBAL, 1, 1), // 3: bump = R1
            abx(op::GETGLOBAL, 2, 1), // 4: R2 = bump
            abc(op::CALL, 2, 1, 2),   // 5: R2 = bump()
            abx(op::GETGLOBAL, 2, 1), // 6
            abc(op::CALL, 2, 1, 2),   // 7: R2 = bump()
            abx(op::GETGLOBAL, 2, 1), // 8
            abc(op::CALL, 2, 1, 2),   // 9: R2 = bump()
            abc(op::RETURN, 2, 2, 0), // 10: return R2
        ],
        vec![n(100.0), s("bump")],
        vec![child],
        0,
        true,
    );
    main.nups = 0;
    main.protos[0].nups = 1;
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out, vec![Value::Num(103.0)]);
}

#[test]
fn vm_concat_and_string_len() {
    let code = vec![
        abx(op::LOADK, 0, 0),       // R0 = "score: "
        abx(op::LOADK, 1, 1),       // R1 = 250
        abc(op::CONCAT, 0, 0, 1),   // R0 = R0 .. R1 = "score: 250"
        abc(op::LEN, 2, 0, 0),      // R2 = #R0 = 10
        abc(op::RETURN, 0, 4, 0),   // return R0, R1, R2 (3 values)
    ];
    let main = proto(code, vec![s("score: "), n(250.0)], vec![], 0, true);
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out[0], Value::Str(rc_bytes("score: 250")));
    assert_eq!(out[2], Value::Num(10.0));
}

/// test helper (avoids repeating Rc::from)
fn rc_bytes(v: &str) -> std::rc::Rc<[u8]> {
    std::rc::Rc::from(v.as_bytes())
}

#[test]
fn vm_host_multi_results_setlist_and_vararg() {
    // host "three" returns 7, 8, 9:
    //   t = { three() }   (CALL C=0 → all results; SETLIST B=0 → top)
    //   return #t, t[3]
    let code = vec![
        abc(op::NEWTABLE, 0, 0, 0),     // 0: R0 = {}
        abx(op::GETGLOBAL, 1, 0),       // 1: R1 = three
        abc(op::CALL, 1, 1, 0),         // 2: R1.. = three()  (all results)
        abc(op::SETLIST, 0, 0, 1),      // 3: R0[1..n] = R1..top
        abc(op::LEN, 2, 0, 0),          // 4: R2 = #R0
        abc(op::GETTABLE, 3, 0, 2),     // 5: R3 = R0[R2] = t[3] = 9
        abc(op::RETURN, 2, 3, 0),       // 6: return R2, R3
    ];
    let main = proto(code, vec![s("three")], vec![], 0, true);
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    vm.register_host("three", |_vm, _args| {
        Ok(vec![Value::Num(7.0), Value::Num(8.0), Value::Num(9.0)])
    })
    .unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out, vec![Value::Num(3.0), Value::Num(9.0)]);
}

#[test]
fn vm_vararg_passthrough() {
    // child(...) collects varargs and returns them all (VARARG B=0,
    // RETURN B=0 — the top plumbing).
    let child = proto(
        vec![
            abc(op::VARARG, 0, 0, 0),  // 0: R0.. = ...
            abc(op::RETURN, 0, 0, 0),  // 1: return R0..top
        ],
        vec![],
        vec![],
        0,
        true, // is_vararg
    );
    // main: pass = child; return pass("a", "b", "c")
    let main = proto(
        vec![
            abx(op::CLOSURE, 0, 0),     // 0: R0 = closure(child)
            abx(op::SETGLOBAL, 0, 0),   // 1: pass = R0
            abx(op::GETGLOBAL, 1, 0),   // 2: R1 = pass
            abx(op::LOADK, 2, 1),       // 3: R2 = "a"
            abx(op::LOADK, 3, 2),       // 4: R3 = "b"
            abx(op::LOADK, 4, 3),       // 5: R4 = "c"
            abc(op::CALL, 1, 4, 0),     // 6: R1.. = pass("a","b","c") (all)
            abc(op::RETURN, 1, 0, 0),   // 7: return R1..top
        ],
        vec![s("pass"), s("a"), s("b"), s("c")],
        vec![child],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    let strs: Vec<String> = out.iter().map(|v| v.display()).collect();
    assert_eq!(strs, vec!["a", "b", "c"]);
}

#[test]
fn vm_comparisons_eq_lt_le() {
    // Each program: set R0/R1, then <CMP A=0 B=1 C=c>; the following
    // JMP executes when the comparison result == C (landing on the
    // "222" load); the JMP is SKIPPED when result != C (falling into
    // the "111" load).  This encodes the lopcodes.h comment directly:
    //   "if ((R[A] op RK[B]) ~= C) then pc++".
    // 222 ⟺ result == C.
    fn check(cmp: u32, x: f32, y: f32, c: u32) -> f32 {
        let code = vec![
            abx(op::LOADK, 0, 0),
            abx(op::LOADK, 1, 1),
            abc(cmp, 0, 1, c),
            asbx(op::JMP, 0, 2),      // 3: -> 6 (result == C path)
            abx(op::LOADK, 2, 2),     // 4: 111 (result != C path)
            asbx(op::JMP, 0, 1),      // 5: -> 7
            abx(op::LOADK, 2, 3),     // 6: 222 (result == C path)
            abc(op::RETURN, 2, 2, 0), // 7
        ];
        let main = proto(
            code,
            vec![n(x), n(y), n(111.0), n(222.0)],
            vec![],
            0,
            true,
        );
        let f = file_of(main);
        let mut vm = Vm::new(&f).unwrap();
        match &vm.run_main().unwrap()[0] {
            Value::Num(v) => *v,
            other => panic!("bad {other:?}"),
        }
    }
    // EQ: result true ⟺ x == y.  222 ⟺ result == C.
    assert_eq!(check(op::EQ, 4.0, 4.0, 1), 222.0); // true == 1
    assert_eq!(check(op::EQ, 4.0, 4.0, 0), 111.0); // true != 0
    assert_eq!(check(op::EQ, 4.0, 5.0, 0), 222.0); // false == 0
    assert_eq!(check(op::EQ, 4.0, 5.0, 1), 111.0); // false != 1
    // LT: 3 < 9 is true.
    assert_eq!(check(op::LT, 3.0, 9.0, 1), 222.0);
    assert_eq!(check(op::LT, 9.0, 3.0, 0), 222.0); // false == 0
    // LE boundary: 3 <= 3 is true.
    assert_eq!(check(op::LE, 3.0, 3.0, 1), 222.0);
    assert_eq!(check(op::LE, 9.0, 3.0, 0), 222.0);
    // negative expectations (result mismatched with C)
    assert_eq!(check(op::LT, 3.0, 9.0, 0), 111.0);
    assert_eq!(check(op::LE, 9.0, 3.0, 1), 111.0);
}

#[test]
fn vm_test_and_testset_jump_semantics() {
    // truth(x) == C → JMP executes → 222 (see layout in check above).
    fn check_test(val: LuaValue, c: u32) -> f32 {
        let code = vec![
            if matches!(val, LuaValue::Bool(_)) {
                let b = matches!(val, LuaValue::Bool(true));
                abc(op::LOADBOOL, 0, b as u32, 0)
            } else if matches!(val, LuaValue::Num(_)) {
                abx(op::LOADK, 0, 0)
            } else {
                abc(op::LOADNIL, 0, 0, 0)
            },
            abc(op::TEST, 0, 0, c),
            asbx(op::JMP, 0, 2),      // -> 5: truth == C path (222)
            abx(op::LOADK, 2, 1),     // 3: 111 (truth != C path)
            asbx(op::JMP, 0, 1),      // 4: -> 6
            abx(op::LOADK, 2, 2),     // 5: 222 (truth == C path)
            abc(op::RETURN, 2, 2, 0), // 6
        ];
        let main = proto(
            code,
            vec![val.clone(), n(111.0), n(222.0)],
            vec![],
            0,
            true,
        );
        let f = file_of(main);
        let mut vm = Vm::new(&f).unwrap();
        match &vm.run_main().unwrap()[0] {
            Value::Num(v) => *v,
            other => panic!("bad {other:?}"),
        }
    }
    // truth(true)=1: C=1 → equal → JMP → 222; C=0 → 111.
    assert_eq!(check_test(LuaValue::Bool(true), 1), 222.0);
    assert_eq!(check_test(LuaValue::Bool(false), 1), 111.0);
    assert_eq!(check_test(LuaValue::Bool(true), 0), 111.0);
    assert_eq!(check_test(LuaValue::Bool(false), 0), 222.0);
    assert_eq!(check_test(LuaValue::Nil, 0), 222.0); // nil truth=0 == C=0
    assert_eq!(check_test(LuaValue::Num(0.0), 0), 111.0); // 0.0 is TRUTHY

    // TESTSET A B C: match → R[A] := R[B] (fall through); else skip.
    let code = vec![
        abc(op::LOADBOOL, 1, 1, 0),     // 0: R1 = true
        abc(op::TESTSET, 3, 1, 1),      // 1: truth(R1)==1 → R3 = R1
        asbx(op::JMP, 0, 1),            // 2: -> 4 (set path)
        abx(op::LOADK, 3, 0),           // 3: NOT taken on match
        abc(op::RETURN, 3, 2, 0),       // 4: return R3 (true)
    ];
    let main = proto(code, vec![n(0.0)], vec![], 0, true);
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out, vec![Value::Bool(true)]);
}

#[test]
fn vm_self_method_call() {
    // child(self, name): params start at R[0] — R0 = self, R1 = name
    // (PROVEN: callee base = func slot + 1, params from R[0]).
    let child = proto(
        vec![
            abx(op::LOADK, 3, 0),       // 0: R3 = "hi "
            abc(op::MOVE, 4, 1, 0),     // 1: R4 = name (R1!)
            abc(op::CONCAT, 3, 3, 4),   // 2: R3 = "hi "..name
            abx(op::LOADK, 4, 1),       // 3: R4 = "!"
            abc(op::CONCAT, 3, 3, 4),   // 4: R3 ..= "!"
            abc(op::RETURN, 3, 2, 0),   // 5
        ],
        vec![s("hi "), s("!")],
        vec![],
        2, // (self, name)
        false,
    );
    // main: t = {}; t.greet = child; t:greet("world") via SELF
    let code = vec![
        abc(op::NEWTABLE, 0, 0, 0),        // 0: R0 = {}
        abx(op::CLOSURE, 2, 0),            // 1: R2 = closure(child)
        abc(op::SETTABLE, 0, 256, 2),      // 2: R0["greet"] = R2
        abc(op::SELF, 3, 0, 256 + 1),      // 3: R4 = t; R3 = t.greet
        abx(op::LOADK, 5, 2),              // 4: R5 = "world"
        abc(op::CALL, 3, 3, 2),            // 5: R3 = R3(R4, R5)
        abc(op::RETURN, 3, 2, 0),          // 6: return R3
    ];
    let main = proto(
        code,
        vec![s("greet"), s("greet"), s("world")],
        vec![child],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out, vec![Value::Str(rc_bytes("hi world!"))]);
}

#[test]
fn vm_tailcall_returns_to_caller() {
    // double(x) = x * 2 (child 0)
    // via(x) = TAILCALL double(x) (child 1)
    // main: return via(21)
    let double = proto(
        vec![
            abc(op::MUL, 0, 0, 256), // R0 = R0 * 2
            abc(op::RETURN, 0, 2, 0),
        ],
        vec![n(2.0)],
        vec![],
        1,
        false,
    );
    let via = proto(
        vec![
            abx(op::GETGLOBAL, 1, 0),     // R1 = double
            abc(op::MOVE, 2, 0, 0),       // R2 = x
            abc(op::TAILCALL, 1, 2, 0),   // return double(x)
        ],
        vec![s("double")],
        vec![],
        1,
        false,
    );
    let main = proto(
        vec![
            abx(op::CLOSURE, 0, 0),     // 0: R0 = closure(double)
            abx(op::SETGLOBAL, 0, 0),   // 1: double = R0
            abx(op::CLOSURE, 0, 1),     // 2: R0 = closure(via)
            abx(op::SETGLOBAL, 0, 1),   // 3: via = R0
            abx(op::GETGLOBAL, 1, 1),   // 4: R1 = via
            abx(op::LOADK, 2, 2),       // 5: R2 = 21
            abc(op::CALL, 1, 2, 2),     // 6: R1 = via(21)
            abc(op::RETURN, 1, 2, 0),   // 7
        ],
        vec![s("double"), s("via"), n(21.0)],
        vec![double, via],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out, vec![Value::Num(42.0)]);
}

#[test]
fn vm_base_library_pairs_next_math() {
    // t = {10, 20, ["k"] = 30}; sum = 0
    // for key, v in pairs(t) do sum = sum + v end
    // Layout (PROVEN from frevents.lua): body falls through to
    // TFORLOOP; the JMP after TFORLOOP re-enters the body.
    let code = vec![
        abc(op::NEWTABLE, 0, 0, 0),       // 0: R0 = {}
        abx(op::LOADK, 1, 0),             // 1: R1 = 10
        abx(op::LOADK, 2, 1),             // 2: R2 = 20
        abc(op::SETLIST, 0, 2, 1),        // 3: t[1..2] = 10, 20
        abc(op::SETTABLE, 0, 256 + 2, 256 + 3), // 4: t["k"] = 30
        abx(op::LOADK, 3, 4),             // 5: R3 = 0 (sum)
        abx(op::GETGLOBAL, 4, 5),         // 6: R4 = pairs
        abc(op::MOVE, 5, 0, 0),           // 7: R5 = t
        abc(op::CALL, 4, 2, 4),           // 8: R4,R5,R6 = pairs(t)
        asbx(op::JMP, 0, 1),              // 9: -> 11 (TFORLOOP)
        abc(op::ADD, 3, 3, 8),            // 10: sum += v (R8 = 2nd TFORLOOP
                                         //     result — results land at
                                         //     R[A+3], R[A+4] with A=4)
        abc(op::TFORLOOP, 4, 0, 2),       // 11: R7,R8 = next(state, ctl)
        asbx(op::JMP, 0, -3),             // 12: -> 10 (body)
        abc(op::RETURN, 3, 2, 0),         // 13: return sum
    ];
    let main = proto(
        code,
        vec![n(10.0), n(20.0), s("k"), n(30.0), n(0.0), s("pairs")],
        vec![],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out, vec![Value::Num(60.0)]);
    // pairs + next were exercised via the host-call record
    assert!(!vm.host_calls_of("pairs").is_empty());
    assert!(vm.host_calls_of("next").len() >= 3); // 3 entries + 1 terminator
}

#[test]
fn vm_math_library_works() {
    // return math.floor(3.7), math.max(1, 9, 4), math.random(5)
    let code = vec![
        abx(op::GETGLOBAL, 0, 0),        // 0: R0 = math
        abc(op::GETTABLE, 0, 0, 256 + 1), // 1: R0 = math.floor
        abx(op::LOADK, 1, 2),            // 2: R1 = 3.7
        abc(op::CALL, 0, 2, 2),          // 3: R0 = math.floor(3.7)
        abx(op::GETGLOBAL, 1, 0),        // 4: R1 = math
        abc(op::GETTABLE, 1, 1, 256 + 3), // 5: R1 = math.max
        abx(op::LOADK, 2, 4),            // 6: R2 = 1
        abx(op::LOADK, 3, 5),            // 7: R3 = 9
        abx(op::LOADK, 4, 6),            // 8: R4 = 4
        abc(op::CALL, 1, 4, 2),          // 9: R1 = math.max(1, 9, 4)
        abx(op::GETGLOBAL, 2, 0),        // 10
        abc(op::GETTABLE, 2, 2, 256 + 7), // 11: R2 = math.random
        abx(op::LOADK, 3, 8),            // 12: R3 = 5
        abc(op::CALL, 2, 2, 2),          // 13: R2 = math.random(5)
        abc(op::RETURN, 0, 4, 0),        // 14
    ];
    let main = proto(
        code,
        vec![
            s("math"),
            s("floor"),
            n(3.7),
            s("max"),
            n(1.0),
            n(9.0),
            n(4.0),
            s("random"),
            n(5.0),
        ],
        vec![],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let out = vm.run_main().unwrap();
    assert_eq!(out[0], Value::Num(3.0));
    assert_eq!(out[1], Value::Num(9.0));
    match &out[2] {
        Value::Num(r) => assert!((1.0..=5.0).contains(r), "random(5) = {r}"),
        other => panic!("bad {other:?}"),
    }
}

#[test]
fn vm_errors_are_clean_not_panics() {
    // calling a nil global
    let main = proto(
        vec![
            abx(op::GETGLOBAL, 0, 0),
            abc(op::CALL, 0, 1, 1),
            abc(op::RETURN, 0, 1, 0),
        ],
        vec![s("missing")],
        vec![],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let err = vm.run_main().unwrap_err().to_string();
    assert!(err.contains("attempt to call"), "got: {err}");

    // arithmetic on nil
    let main = proto(
        vec![
            abc(op::LOADNIL, 0, 0, 0),
            abc(op::ADD, 1, 0, 256),
            abc(op::RETURN, 0, 1, 0),
        ],
        vec![n(1.0)],
        vec![],
        0,
        true,
    );
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    let err = vm.run_main().unwrap_err().to_string();
    assert!(err.contains("arithmetic"), "got: {err}");

    // infinite loop → instruction budget
    let main = proto(vec![asbx(op::JMP, 0, -1)], vec![], vec![], 0, true);
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    vm.max_steps = 100;
    let err = vm.run_main().unwrap_err().to_string();
    assert!(err.contains("budget"), "got: {err}");

    // runaway recursion → depth cap
    // f = function() return f() end  → stack overflow
    let child = proto(
        vec![
            abx(op::GETGLOBAL, 1, 0),
            abc(op::CALL, 1, 1, 2),
            abc(op::RETURN, 1, 2, 0),
        ],
        vec![s("f")],
        vec![],
        0,
        false,
    );
    let mut main = proto(
        vec![
            abx(op::CLOSURE, 0, 0),
            abx(op::SETGLOBAL, 0, 0),
            abx(op::GETGLOBAL, 1, 0),
            abc(op::CALL, 1, 1, 2),
            abc(op::RETURN, 1, 2, 0),
        ],
        vec![s("f")],
        vec![child],
        0,
        true,
    );
    main.protos[0].nups = 0;
    let f = file_of(main);
    let mut vm = Vm::new(&f).unwrap();
    vm.max_depth = 64;
    let err = vm.run_main().unwrap_err().to_string();
    assert!(err.contains("stack overflow"), "got: {err}");
}

// ===========================================================================
// Real-fixture acceptance runs
// ===========================================================================

/// frevents.lua: run the chunk, verify the registration flow.
#[test]
fn vm_frevents_registers_events_and_collectibles() {
    let data = fixture("frevents.lua");
    let f = Lua51File::parse(&data).unwrap();
    let mut vm = Vm::new(&f).unwrap();
    stub_hosts(
        &mut vm,
        &[
            "fre_RegisterEvent",
            "fre_RegisterCollectibles",
            "fre_ResetEvent",
            "fre_DestroyEvent",
        ],
    );
    vm.run_main().unwrap();

    // ---- fre_RegisterEvent: one call per event-def table -------------
    let events = vm.host_calls_of("fre_RegisterEvent");
    // STATIC expectation: the SETLIST arithmetic in the main proto
    // (B=50 C=1 then B=10 C=2) predicts 60 defs.
    let static_count: usize = f
        .main
        .code
        .iter()
        .map(|&raw| crate::Instruction::decode(raw))
        .filter(|i| i.opcode == 34 && i.b != 0) // SETLIST with explicit B
        .map(|i| i.b as usize)
        .sum();
    assert_eq!(static_count, 60, "static SETLIST prediction");
    assert_eq!(
        events.len(),
        static_count,
        "dynamic fre_RegisterEvent count must equal the static prediction"
    );

    // every arg is a def table with string "name" and "class" fields;
    // record both (EVIDENCE: recovered at runtime, cross-checked against
    // the constant pool).
    let mut names: Vec<String> = Vec::new();
    let mut classes: Vec<String> = Vec::new();
    for (i, args) in events.iter().enumerate() {
        match args.first() {
            Some(Value::Table(t)) => {
                let key = |k: &str| {
                    t.borrow().get(&Value::Str(rc_bytes(k)))
                };
                match key("name") {
                    Some(Value::Str(s)) => {
                        names.push(String::from_utf8_lossy(&s).into_owned())
                    }
                    other => panic!("def #{i}: bad name field {other:?}"),
                }
                match key("class") {
                    Some(Value::Str(s)) => {
                        classes.push(String::from_utf8_lossy(&s).into_owned())
                    }
                    other => panic!("def #{i}: bad class field {other:?}"),
                }
            }
            other => panic!("fre_RegisterEvent #{i}: bad arg {other:?}"),
        }
    }

    // PROVEN event families and classes (runtime + constant pool agree):
    //   8 freP + 9 freW + 11 freV + 5 freF + 20 freL + 6 freO + 1 freC = 60
    //   classes: freKill (freP/freV/freF), fre3WW (freW), freLoco (freL00-14),
    //            freGlide (freL20-29), freOpp (freO), freColl (freC00)
    assert_eq!(names.first().map(String::as_str), Some("freP00"));
    assert_eq!(classes.first().map(String::as_str), Some("freKill"));
    assert_eq!(names.last().map(String::as_str), Some("freC00"));
    assert_eq!(classes.last().map(String::as_str), Some("freColl"));
    let mut distinct_classes: Vec<String> = classes.clone();
    distinct_classes.sort();
    distinct_classes.dedup();
    assert_eq!(
        distinct_classes,
        vec![
            "fre3WW".to_string(),
            "freColl".to_string(),
            "freGlide".to_string(),
            "freKill".to_string(),
            "freLoco".to_string(),
            "freOpp".to_string(),
        ]
    );
    // The three-way-war class rides the freW family.
    for (name, class) in names.iter().zip(classes.iter()) {
        assert_eq!(name.starts_with("fre"), true, "def name {name}");
        if name.starts_with("freW") {
            assert_eq!(class, "fre3WW");
        }
        if name.starts_with("freL2") {
            assert_eq!(class, "freGlide");
        }
    }

    // ---- fre_RegisterCollectibles(collectiblesDef global) -------------
    let coll = vm.host_calls_of("fre_RegisterCollectibles");
    assert_eq!(coll.len(), 1);
    let arg = coll[0][0].clone();
    let def_table = match &arg {
        Value::Table(t) => t.clone(),
        other => panic!("collectiblesDef arg: {other:?}"),
    };
    let kv = |k: &str| {
        def_table
            .borrow()
            .get(&Value::Str(rc_bytes(k)))
            .expect("field {k}")
    };
    assert_eq!(kv("maxHintCollectibles"), Value::Num(50.0));
    assert_eq!(kv("maxLocationCollectibles"), Value::Num(500.0));
    assert_eq!(kv("numScientistEvents"), Value::Num(5.0));
    // PROVEN structure: the global holds exactly the scalar fields; the
    // 60 event defs live in a local (see PROGRESS M8).
    assert_eq!(def_table.borrow().len(), 3);
    // The global the VM exposes is the SAME object (reference semantics).
    let g = vm.get_global("collectiblesDef");
    match (&g, &arg) {
        (Value::Table(a), Value::Table(b)) => {
            assert!(std::rc::Rc::ptr_eq(a, b));
        }
        _ => panic!("collectiblesDef not a table"),
    }
}

#[test]
fn vm_frevents_lifecycle_closures_iterate_all_defs() {
    let data = fixture("frevents.lua");
    let f = Lua51File::parse(&data).unwrap();
    let mut vm = Vm::new(&f).unwrap();
    stub_hosts(
        &mut vm,
        &[
            "fre_RegisterEvent",
            "fre_RegisterCollectibles",
            "fre_ResetEvent",
            "fre_DestroyEvent",
        ],
    );
    vm.run_main().unwrap();

    // The script-defined lifecycle globals are closures.
    for name in ["ResetFreeRoamEvents", "DestroyFreeRoamEvents"] {
        assert!(
            matches!(vm.get_global(name), Value::Closure(_)),
            "{name} must be a closure after the run"
        );
    }
    // Calling them walks the same 60 defs (upvalue capture works).
    vm.call_global("ResetFreeRoamEvents", &[]).unwrap();
    assert_eq!(vm.host_calls_of("fre_ResetEvent").len(), 60);
    vm.call_global("DestroyFreeRoamEvents", &[]).unwrap();
    assert_eq!(vm.host_calls_of("fre_DestroyEvent").len(), 60);
    // Each reset/destroy arg is the SAME def table object that was
    // registered (proves the pairs iteration yields identical values).
    let registered: Vec<Value> = vm
        .host_calls_of("fre_RegisterEvent")
        .iter()
        .map(|a| a[0].clone())
        .collect();
    let reset: Vec<Value> =
        vm.host_calls_of("fre_ResetEvent").iter().map(|a| a[0].clone()).collect();
    assert_eq!(registered, reset, "reset must see the same def tables");
}

#[test]
fn vm_mission_script_calls_add_valid_mission_70_times() {
    // Extract the 2318-byte mission chunk from scripts.rcf.
    let rcf = fixture("scripts.rcf");
    let arc = prototype_rcf::RcfArchive::parse(&rcf).unwrap();
    let mut chunk = None;
    for e in &arc.entries {
        let p = arc.payload(e).unwrap();
        if prototype_rcf::PayloadKind::sniff(p)
            == prototype_rcf::PayloadKind::Lua51
            && p.len() == 2318
        {
            chunk = Some(p.to_vec());
        }
    }
    let data = chunk.expect("mission chunk in scripts.rcf");
    let f = Lua51File::parse(&data).unwrap();
    let mut vm = Vm::new(&f).unwrap();
    vm.register_host("AddValidMission", |_vm, _args| Ok(Vec::new()))
        .unwrap();
    vm.register_host("util_GetRandomFloat", |_vm, _args| {
        Ok(vec![Value::Num(0.5)])
    })
    .unwrap();
    vm.run_main().unwrap();

    // Dynamic count == static GETGLOBAL census (70 — PROVEN in M5).
    let static_calls = f
        .main
        .globals_read()
        .iter()
        .filter(|g| **g == *b"AddValidMission")
        .count();
    assert_eq!(static_calls, 70);
    let calls = vm.host_calls_of("AddValidMission");
    assert_eq!(
        calls.len(),
        static_calls,
        "dynamic AddValidMission count must equal the static census"
    );
    // First and last registrations (constant pool order, PROVEN).
    let first = &calls[0];
    assert_eq!(first[0], Value::Str(rc_bytes("Infected_Test")));
    assert_eq!(first[1], Value::Str(rc_bytes("manhattan")));
    let last = &calls[69];
    assert_eq!(last[0], Value::Str(rc_bytes("e11m01")));
    assert_eq!(last[1], Value::Str(rc_bytes("manhattan")));

    // math.randomseed(util_GetRandomFloat() * 10000) = randomseed(5000)
    let seeds = vm.host_calls_of("math.randomseed");
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0][0], Value::Num(5000.0));
}
