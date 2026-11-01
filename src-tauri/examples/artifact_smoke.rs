//! Manual smoke test: real engine + real Trash on the throwaway project ~/development/deskveet-art-test.
use deskveet_lib::cleaner::{Engine, RunOpts};
use deskveet_lib::rules::Registry;
use deskveet_lib::sys::RealSys;
use std::sync::{atomic::{AtomicBool, AtomicU64}, Arc};

fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let root = home.join("development/deskveet-art-test");
    let undo = std::env::temp_dir().join(format!("deskveet-art-undo-{}.json", std::process::id()));
    let eng = Engine::new(Registry::macos().unwrap(), home.clone(), Arc::new(RealSys::new(home.clone())), home.join(".Trash"), undo.clone());
    let (scan, results) = eng.find_artifacts(&[root.clone()], &Default::default(), &AtomicBool::new(false), &AtomicU64::new(0));
    println!("found {} project(s): {:?}", scan.projects.len(), scan.projects.iter().map(|p| (&p.name, p.total, p.items.iter().map(|i| i.dir.clone()).collect::<Vec<_>>())).collect::<Vec<_>>());
    let ids: Vec<String> = results.iter().flat_map(|r| r.items.iter().map(|i| i.id.clone())).collect();
    let plan = eng.plan(&results, &ids, &[], false);
    for i in &plan.items { println!("plan: {} blocked={:?} confirm={:?} method={:?}", i.label, i.blocked, i.confirm, i.method); }
    let mut opts = RunOpts::default();
    opts.overrides = plan.items.iter().map(|i| i.id.clone()).collect(); // the user ticked the "node is running" box
    let rep = eng.run(&plan, &opts, &|_, _, _| {});
    println!("run: ok={} skipped={} failed={} trashed={}", rep.ok, rep.skipped, rep.failed, rep.trashed_bytes);
    println!("node_modules gone: {}; index.js and package.json kept: {}", !root.join("proj/node_modules").exists(), root.join("proj/index.js").exists() && root.join("proj/package.json").exists());
    let e = eng.undo.list().remove(0);
    println!("trash_path={:?}", e.trash_path);
    let back = eng.restore(&e.id).expect("restore");
    println!("restored: {back}; exists: {}", root.join("proj/node_modules/lib/blob.js").exists());
    let _ = std::fs::remove_file(undo);
}
