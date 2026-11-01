//! Manual smoke test of the REAL Trash path on a throwaway file this program creates itself.
use deskveet_lib::cleaner::{Engine, RunOpts};
use deskveet_lib::rules::Registry;
use deskveet_lib::sys::RealSys;
use std::sync::Arc;

fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let work = home.join(format!("deskveet-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let file = work.join("throwaway-smoke-test.bin");
    std::fs::write(&file, vec![1u8; 3 * 1024 * 1024]).unwrap();
    let undo = std::env::temp_dir().join(format!("deskveet-smoke-undo-{}.json", std::process::id()));
    let eng = Engine::new(Registry::macos().unwrap(), home.clone(), Arc::new(RealSys::new(home.clone())), home.join(".Trash"), undo.clone());

    let plan = eng.plan(&[], &[], &[file.to_string_lossy().into_owned(), work.to_string_lossy().into_owned() + "/../Documents"], false);
    println!("plan: {} items", plan.items.len());
    for i in &plan.items {
        println!("  {} -> blocked={:?} method={:?}", i.label, i.blocked, i.method);
    }
    let rep = eng.run(&plan, &RunOpts::default(), &|_, _, _| {});
    println!("run: ok={} skipped={} failed={} trashed={} bytes", rep.ok, rep.skipped, rep.failed, rep.trashed_bytes);
    println!("original gone: {}", !file.exists());
    let e = eng.undo.list().remove(0);
    println!("log: action={} status={} trash_path={:?}", e.action, e.status, e.trash_path);
    let tp = e.trash_path.clone().expect("trash path discovered");
    println!("in Trash: {}", std::path::Path::new(&tp).exists());
    let back = eng.restore(&e.id).expect("restore");
    println!("restored to {back}; exists again: {}", file.exists());
    std::fs::remove_dir_all(&work).unwrap();
    let _ = std::fs::remove_file(undo);
    println!("cleaned up scratch folder {}", work.display());
}
