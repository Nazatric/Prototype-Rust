use prototype_lua::{Lua51File, Value, Vm};
fn main() {
    let data = std::fs::read("crates/prototype-bin/tests/fixtures/scripts.rcf").unwrap();
    let arc = prototype_rcf::RcfArchive::parse(&data).unwrap();
    let mut chunk = None;
    for e in &arc.entries {
        let p = arc.payload(e).unwrap();
        if prototype_rcf::PayloadKind::sniff(p) == prototype_rcf::PayloadKind::Lua51 && p.len() == 2318 { chunk = Some(p.to_vec()); }
    }
    let f = Lua51File::parse(&chunk.unwrap()).unwrap();
    let mut vm = Vm::new(&f).unwrap();
    vm.register_host("AddValidMission", |_v, _a| Ok(vec![])).unwrap();
    vm.register_host("util_GetRandomFloat", |_v, _a| Ok(vec![Value::Num(0.5)])).unwrap();
    vm.run_main().unwrap();
    let mut zones: Vec<(String, usize)> = vec![];
    for args in vm.host_calls_of("AddValidMission") {
        let z = args[1].display();
        match zones.iter_mut().find(|(n, _)| *n == z) { Some(e) => e.1 += 1, None => zones.push((z, 1)) }
    }
    println!("{:?}", zones);
    // print the non-manhattan ones
    for args in vm.host_calls_of("AddValidMission") {
        if args[1].display() != "manhattan" { println!("  {} ({})", args[0].display(), args[1].display()); }
    }
}
