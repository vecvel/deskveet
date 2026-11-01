//! Dev helper: prints real Overview + ProcList JSON (used to render the UI in a plain browser).
use deskveet_lib::sysmon::{Monitor, ProcQuery};

fn main() {
    let mut m = Monitor::new();
    let _ = m.overview();
    std::thread::sleep(std::time::Duration::from_millis(900));
    let o = m.overview();
    let p = m.processes(&ProcQuery { sort: "mem".into(), limit: 40, query: String::new() });
    println!("{}", serde_json::json!({ "overview": o, "processes": p }));
}
