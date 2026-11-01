//! Dev helper: read-only artifact scan of folders given as arguments (default: Desktop + development).
use deskveet_lib::cleaner::Engine;
use deskveet_lib::rules::Registry;
use deskveet_lib::sys::RealSys;
use std::sync::{atomic::{AtomicBool, AtomicU64}, Arc};

fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let mut roots: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    if roots.is_empty() {
        roots = ["Desktop", "development"].iter().map(|d| home.join(d)).filter(|p| p.is_dir()).collect();
    }
    let eng = Engine::new(Registry::macos().unwrap(), home.clone(), Arc::new(RealSys::new(home.clone())), home.join(".Trash"), std::env::temp_dir().join("deskveet-art-undo.json"));
    let t = std::time::Instant::now();
    let (scan, _) = eng.find_artifacts(&roots, &Default::default(), &AtomicBool::new(false), &AtomicU64::new(0));
    println!("roots {:?}\n{} projects, {:.2} GB in {:.1}s ({} dirs walked)", scan.roots, scan.projects.len(), scan.total as f64 / 1e9, t.elapsed().as_secs_f64(), scan.dirs);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    for p in scan.projects.iter().take(15) {
        println!("{:>8.2} GB  {:>4}d idle  {}  [{}]", p.total as f64 / 1e9, (now - p.last_active) / 86400, p.path.replace(home.to_str().unwrap(), "~"), p.items.iter().map(|i| i.dir.as_str()).collect::<Vec<_>>().join(", "));
    }
    let stale: u64 = scan.projects.iter().filter(|p| now - p.last_active > 60 * 86400).map(|p| p.total).sum();
    println!("older than 60 days: {:.2} GB", stale as f64 / 1e9);
}
