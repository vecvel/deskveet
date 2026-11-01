//! Dev helper: real Apps tab rows (running apps, memory, CPU, storage, icons) as JSON.
use deskveet_lib::apps::{base64, data_paths, protected_reason, AppRow};
use deskveet_lib::fsutil::measure;
use deskveet_lib::platform::{self, Apps};
use deskveet_lib::sysmon::Monitor;

fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let plat = platform::current();
    let mut mon = Monitor::new();
    let apps = plat.running_apps();
    let _ = mon.app_usage(&apps);
    std::thread::sleep(std::time::Duration::from_millis(800));
    let usage = mon.app_usage(&apps);
    let t = std::time::Instant::now();
    let rows: Vec<AppRow> = apps.iter().zip(usage).map(|(a, (mem, cpu, procs))| {
        let bundle = a.bundle_path.as_deref().map(|p| measure(p).bytes);
        let data = data_paths(&home, &a.bundle_id, &a.name).iter().map(|p| measure(p).bytes).sum::<u64>();
        AppRow { pid: a.pid, name: a.name.clone(), bundle_id: a.bundle_id.clone(), path: a.bundle_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            mem, cpu, procs, bundle_bytes: bundle, data_bytes: Some(data), protected: protected_reason(a, std::process::id()),
            icon: a.bundle_path.as_deref().and_then(|p| plat.app_icon_png(p)).map(|png| format!("data:image/png;base64,{}", base64(&png))) }
    }).collect();
    eprintln!("{} apps, sizes measured in {:.1}s", rows.len(), t.elapsed().as_secs_f64());
    for r in &rows { eprintln!("{:<28} {:>7.0} MB  {:>3} procs  cpu {:>5.1}%  storage {:>7.2} GB  icon {}  {}", r.name, r.mem as f64/1048576.0, r.procs, r.cpu, (r.bundle_bytes.unwrap_or(0)+r.data_bytes.unwrap_or(0)) as f64/1e9, r.icon.as_ref().map(|i| i.len()/1024).unwrap_or(0), r.protected.unwrap_or("")); }
    println!("{}", serde_json::to_string(&rows).unwrap());
}
