//! Dev helper: every rule's status on this Mac. `--json` prints {rules, status, locations} for the UI mock.
use deskveet_lib::cleaner::{Engine, RuleInfo};
use deskveet_lib::rules::Registry;
use deskveet_lib::sys::RealSys;
use std::sync::Arc;
fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let eng = Engine::new(Registry::macos().unwrap(), home.clone(), Arc::new(RealSys::new(home.clone())), home.join(".Trash"), std::env::temp_dir().join("x.json"));
    let t = std::time::Instant::now();
    let all = eng.status_all();
    if std::env::args().any(|a| a == "--json") {
        let rules: Vec<RuleInfo> = eng.reg.rules.iter().map(Into::into).collect();
        let mut locs: Vec<_> = eng.locations().values().cloned().collect();
        locs.sort_by(|a, b| a.id.cmp(&b.id));
        println!("{}", serde_json::json!({ "rules": rules, "status": all, "locations": locs }));
        return;
    }
    println!("{} rules in {:.2}s", all.len(), t.elapsed().as_secs_f64());
    for s in all { println!("{:<28} {:<16} {:>4}  {}", s.id, s.state, s.matches, s.note.unwrap_or_default()); }
}
