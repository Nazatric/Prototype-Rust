//! run-frevents — debug: run frevents.lua in the VM and dump every
//! host call + the collectiblesDef table.

use prototype_lua::{Lua51File, Value, Vm};

fn dump_table(t: &std::rc::Rc<std::cell::RefCell<prototype_lua::Table>>, indent: usize) {
    let tb = t.borrow();
    for (k, v) in tb.iter() {
        let pad = "  ".repeat(indent);
        match v {
            Value::Table(inner) => {
                println!("{pad}{} => ({} entries)", k.display(), inner.borrow().len());
                dump_table(inner, indent + 1);
            }
            _ => println!("{pad}{} = {}", k.display(), v.display()),
        }
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "crates/prototype-bin/tests/fixtures/frevents.lua".into());
    let data = std::fs::read(&path).unwrap();
    let f = Lua51File::parse(&data).unwrap();
    let mut vm = Vm::new(&f).unwrap();
    for name in [
        "fre_RegisterEvent",
        "fre_RegisterCollectibles",
        "fre_ResetEvent",
        "fre_DestroyEvent",
    ] {
        vm.register_host(name, |_vm, _args| Ok(Vec::new())).unwrap();
    }
    vm.run_main().unwrap();

    println!("== host call counts ==");
    let mut names: Vec<String> = Vec::new();
    for call in &vm.host_calls {
        println!("  {} ({})", call.name, call.args.len());
    }
    println!("== fre_RegisterEvent names ==");
    for args in vm.host_calls_of("fre_RegisterEvent") {
        if let Some(Value::Table(t)) = args.first() {
            let name = t.borrow().get(&Value::Str(std::rc::Rc::from(b"name".as_slice())));
            let class = t.borrow().get(&Value::Str(std::rc::Rc::from(b"class".as_slice())));
            let name = name.map(|v| v.display()).unwrap_or_else(|| "<no name>".into());
            let class = class.map(|v| v.display()).unwrap_or_else(|| "<no class>".into());
            names.push(name.clone());
            println!("  name={name} class={class}");
        }
    }
    println!("total: {}", names.len());
    println!("== collectiblesDef global ==");
    if let Value::Table(t) = vm.get_global("collectiblesDef") {
        dump_table(&t, 1);
    }
}
