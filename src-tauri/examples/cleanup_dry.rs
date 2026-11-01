//! Dev helper: scan every rule on this Mac (read-only) and build a dry-run plan of the "safe" ones.
use deskveet_lib::cleaner::{Engine, RunOpts};
use deskveet_lib::rules::{Registry, Risk};
use deskveet_lib::sys::RealSys;
use std::sync::{atomic::AtomicBool, Arc};

fn gb(b: u64) -> String {
    format!("{:>7.2} GB", b as f64 / 1e9)
}

fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let undo = std::env::temp_dir().join("deskveet-example-undo.json");
    let _ = std::fs::remove_file(&undo);
    let eng = Engine::new(Registry::macos().unwrap(), home.clone(), Arc::new(RealSys::new(home.clone())), home.join(".Trash"), undo.clone());
    let t = std::time::Instant::now();
    let results = eng.scan_all(&AtomicBool::new(false), &Default::default(), &|_| {});
    if let Some(out) = std::env::var_os("DUMP_RESULTS") {
        std::fs::write(out, serde_json::to_vec(&results).unwrap()).unwrap();
    }
    println!("scanned {} rules in {:.1}s\n", results.len(), t.elapsed().as_secs_f64());
    let mut total = 0;
    for r in &results {
        let blocked = r.items.iter().filter(|i| i.blocked.is_some()).count();
        println!("{:<28} {:<8?} {} {:>4} items{}{}{}", r.rule.id, r.rule.risk, gb(r.total), r.items.len(),
            if blocked > 0 { format!("  ({blocked} blocked)") } else { String::new() },
            r.note.as_ref().map(|n| format!("  [{n}]")).unwrap_or_default(),
            if r.running.is_empty() { String::new() } else { format!("  running: {:?}", r.running) });
        total += r.total;
        for i in r.items.iter().take(2) {
            println!("      {} {}{}", gb(i.size), i.label, i.blocked.as_ref().map(|b| format!("   BLOCKED: {b}")).unwrap_or_default());
        }
    }
    println!("\nTOTAL reclaimable (all rules): {}", gb(total));
    let safe: Vec<String> = results.iter().filter(|r| r.rule.risk == Risk::Safe && !r.rule.opt_in).flat_map(|r| r.items.iter().filter(|i| i.blocked.is_none()).map(|i| i.id.clone())).collect();
    let t = std::time::Instant::now();
    let plan = eng.plan(&results, &safe, &[], true);
    println!("dry-run plan for {} safe items built in {:.1}s: total {}, {} need confirmation, {} blocked, erase={}", plan.items.len(), t.elapsed().as_secs_f64(), gb(plan.total),
        plan.items.iter().filter(|i| !i.confirm.is_empty()).count(), plan.items.iter().filter(|i| i.blocked.is_some()).count(), plan.erase);
    for i in plan.items.iter().filter(|i| i.blocked.is_some() || !i.confirm.is_empty()).take(6) {
        println!("   - {} :: {:?} {:?}", i.label, i.blocked, i.confirm);
    }
    let rep = eng.run(&plan, &RunOpts::default(), &|_, _, _| {});
    println!("dry-run report: ok {} skipped {} failed {} (nothing was changed)", rep.ok, rep.skipped, rep.failed);
    let _ = std::fs::remove_file(undo);
}
