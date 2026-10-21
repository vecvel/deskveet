//! Dev helper: read-only scan of a folder (default $HOME), cold then incremental, with peak RSS.
use deskveet_lib::scan::{self, Ctx, Opts};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Instant;

fn peak_rss_mb() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    ru.ru_maxrss as f64 / 1024.0 / 1024.0 // bytes on macOS
}

fn main() {
    let root = std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(|| std::env::var_os("HOME").unwrap().into());
    let dev = scan::dev_of(&root).unwrap();
    let idx_path = std::env::temp_dir().join("deskveet-bench.idx");
    let go = |prev| {
        let w = scan::IdxWriter::create(&idx_path).unwrap();
        let ctx = Ctx::new(Arc::new(AtomicBool::new(false)), prev, dev, Opts::default(), Some(w));
        let t = Instant::now();
        let tree = scan::run(&root, &ctx).unwrap();
        ctx.idx.as_ref().unwrap().finish(true).unwrap();
        (tree, ctx, t.elapsed())
    };
    let (tree, ctx, dt) = go(None);
    let o = std::sync::atomic::Ordering::Relaxed;
    println!("cold:        {:>6.2}s  {} files  {:.1} GB  {} dirs  denied {} (protected {})  peak RSS {:.0} MB", dt.as_secs_f64(), tree.files, tree.size as f64 / 1e9, ctx.dirs.load(o), ctx.denied_n.load(o), ctx.protected_n.load(o), peak_rss_mb());
    let kept = { fn c(n: &scan::Node) -> usize { 1 + n.kids.iter().map(c).sum::<usize>() } c(&tree) };
    println!("kept nodes:  {kept}   tree json {:.1} MB   index file {:.1} MB", serde_json::to_vec(&tree).unwrap().len() as f64 / 1e6, std::fs::metadata(&idx_path).unwrap().len() as f64 / 1e6);
    for k in tree.kids.iter().take(8) {
        println!("  {:>8.2} GB  {}", k.size as f64 / 1e9, k.name);
    }
    if let Some(out) = std::env::var_os("DUMP_TREE") {
        std::fs::write(out, serde_json::to_vec(&tree).unwrap()).unwrap();
    }
    drop(tree);
    let t = Instant::now();
    let prev = scan::read_index(&idx_path).unwrap();
    println!("index load:  {:>6.2}s  {} dirs  peak RSS {:.0} MB", t.elapsed().as_secs_f64(), prev.len(), peak_rss_mb());
    let (t2, ctx2, dt2) = go(Some(prev));
    println!("incremental: {:>6.2}s  {:.1} GB  reused {} of {} dirs  peak RSS {:.0} MB", dt2.as_secs_f64(), t2.size as f64 / 1e9, ctx2.reused.load(o), ctx2.dirs.load(o), peak_rss_mb());
    let _ = std::fs::remove_file(idx_path);
}
