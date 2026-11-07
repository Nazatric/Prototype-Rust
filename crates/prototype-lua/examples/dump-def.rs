#![allow(dead_code)] // example helper
// quick inline example: dump the first + last fre_RegisterEvent def fully
use prototype_lua::{Lua51File, Value, Vm};
fn dump(t: &std::rc::Rc<std::cell::RefCell<prototype_lua::Table>>, ind: usize) {
    for (k, v) in t.borrow().iter() {
        let pad = "  ".repeat(ind);
        match v {
            Value::Table(inner) => { println!("{pad}{}:", k.display()); dump(inner, ind+1); }
            _ => println!("{pad}{} = {}", k.display(), v.display()),
        }
    }
}
fn main() {
    let data = std::fs::read("crates/prototype-bin/tests/fixtures/frevents.lua").unwrap();
    let f = Lua51File::parse(&data).unwrap();
    let mut vm = Vm::new(&f).unwrap();
    for n in ["fre_RegisterEvent","fre_RegisterCollectibles","fre_ResetEvent","fre_DestroyEvent"] {
        vm.register_host(n, |_v,_a| Ok(vec![])).unwrap();
    }
    vm.run_main().unwrap();
    let defs = vm.host_calls_of("fre_RegisterEvent");
    // exhaustive key census over all 60 defs + all var-defs
    let mut def_keys = std::collections::BTreeMap::new();
    let mut var_keys = std::collections::BTreeMap::new();
    let mut var_names = std::collections::BTreeSet::new();
    let mut n_vars = 0;
    for a in &defs {
        if let Some(Value::Table(t)) = a.first() {
            for (k, v) in t.borrow().iter() {
                *def_keys.entry(k.display()).or_insert(0usize) += 1;
                if k.display() == "vars" {
                    if let Value::Table(vt) = v {
                        for (vk, vv) in vt.borrow().iter() {
                            var_names.insert(vk.display());
                            n_vars += 1;
                            if let Value::Table(vd) = vv {
                                for (dk, _) in vd.borrow().iter() {
                                    *var_keys.entry(dk.display()).or_insert(0usize) += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let mut counts = std::collections::BTreeMap::new();
    for a in &defs { if let Some(Value::Table(t)) = a.first() { let g = |k: &str| t.borrow().get(&Value::Str(std::rc::Rc::from(k.as_bytes()))); let n = g("name").unwrap().display(); let c = g("class").unwrap().display(); println!("{n} {c}"); *counts.entry(c).or_insert(0usize) += 1; } }
    println!("counts: {counts:?}");
    println!("== var-def key census ({n_vars} vars) ==\n{var_keys:?}");
    println!("== all var names ==\n{var_names:?}");
}
