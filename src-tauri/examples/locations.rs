//! Dev helper: show where every tool location resolved to on this Mac, and from which source.
use deskveet_lib::cleaner::Engine;
use deskveet_lib::rules::Registry;
use deskveet_lib::sys::RealSys;
use std::sync::Arc;

fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let eng = Engine::new(Registry::macos().unwrap(), home.clone(), Arc::new(RealSys::new(home.clone())), home.join(".Trash"), std::env::temp_dir().join("x.json"));
    let t = std::time::Instant::now();
    let locs = eng.locations();
    println!("resolved {} locations in {:.2}s", locs.len(), t.elapsed().as_secs_f64());
    let mut v: Vec<_> = locs.values().collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    for r in v {
        let p = r.path.as_ref().map(|p| p.to_string_lossy().replace(home.to_str().unwrap(), "~")).unwrap_or_else(|| "-".into());
        println!("{:<16} {:<22} {}{}", r.id, r.source, p, if r.skipped.is_empty() { String::new() } else { format!("   skipped: {:?}", r.skipped) });
    }
}
