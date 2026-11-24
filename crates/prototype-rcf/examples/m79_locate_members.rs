//! Block 79 RECOVERY: locate + decompress the lost block-77 NIS
//! fixtures from the live art.rcf release, surgically.
//!
//! SUBCOMMANDS (argv[1]):
//!   locate <art_head256.bin>          — parse the 256 KB directory
//!     prefix, hash the three known member names, print their spans.
//!   cut <art_head256.bin> <member.rz> <name> [<name>...] — same parse,
//!     then extract the named members' RAW payloads into files.
//!   rz <in.rz> <out.bin>...            — RZ-decompress files pairwise.
//!
//! The proven `parse_directory` + `hash_name` + `rz::decompress` paths
//! do all the real work (bit-identical to the whole-file parse, per
//! the crate's own tests).

use std::io::Cursor;

const MEMBERS: [&str; 3] = [
    "art\\nis\\NIS_0_3_a0.p3d.rz",
    "art\\nis\\NIS_0_3_a1.p3d.rz",
    "art\\nis\\NIS_0_3_a.p3d.rz",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args[1].as_str() {
        "locate" => {
            let head = std::fs::read(&args[2]).expect("head file");
            let mut cur = Cursor::new(head);
            let (header, entries) =
                prototype_rcf::parse_directory(&mut cur).expect("directory parse");
            println!(
                "dir: {} entries, dir_offset {}, dir_size {}",
                entries.len(),
                header.dir_offset,
                header.dir_size_in_bytes
            );
            for name in MEMBERS {
                let h = prototype_rcf::hash_name(name);
                match entries.iter().find(|e| e.name_hash == h) {
                    Some(e) => println!("FOUND {name} hash {h:08x} offset {} size {}", e.file_offset, e.file_size),
                    None => println!("MISSING {name} hash {h:08x}"),
                }
            }
        }
        "rz" => {
            // pairwise in.rz -> out.bin
            let mut i = 2;
            while i + 1 < args.len() {
                let raw = std::fs::read(&args[i]).expect("member file");
                println!("{}: {} B raw, rz={}", args[i], raw.len(), prototype_rcf::rz::is_rz(&raw));
                let out = prototype_rcf::rz::decompress(&raw).expect("rz decompress");
                std::fs::write(&args[i + 1], &out).expect("write out");
                println!("  -> {} ({} B)", args[i + 1], out.len());
                i += 2;
            }
        }
        other => panic!("unknown subcommand {other}"),
    }
}
