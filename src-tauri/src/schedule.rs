//! Alerts and scheduled cleaning: configuration and the pure "is it due?" logic.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Every {
    Daily,
    Weekly,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Schedule {
    pub every: Every,
    /// Local hour 0-23.
    pub hour: u8,
    /// 0 = Sunday .. 6 = Saturday (weekly only).
    #[serde(default)]
    pub weekday: u8,
    /// The user agreed that a *review* rule may be cleaned automatically.
    #[serde(default)]
    pub review_ok: bool,
    /// When the schedule was set (a new schedule never runs for a time already passed).
    #[serde(default)]
    pub created: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    /// Rule id -> alert when the rule's size reaches this many bytes.
    #[serde(default)]
    pub alerts: HashMap<String, u64>,
    /// Alert when the start-up disk has less than this percentage free (None = off).
    #[serde(default = "default_disk")]
    pub disk_free_pct: Option<f32>,
    #[serde(default)]
    pub schedules: HashMap<String, Schedule>,
    /// Alert key -> last time it was shown (unix seconds).
    #[serde(default)]
    pub last_alert: HashMap<String, u64>,
    /// Rule id -> last scheduled run (unix seconds).
    #[serde(default)]
    pub last_run: HashMap<String, u64>,
}

fn default_disk() -> Option<f32> {
    Some(10.0)
}

pub struct Store {
    path: PathBuf,
    pub cfg: Mutex<Config>,
}

impl Store {
    pub fn load(path: PathBuf) -> Store {
        let cfg = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Config { disk_free_pct: default_disk(), ..Default::default() });
        Store { path, cfg: Mutex::new(cfg) }
    }

    pub fn save(&self) {
        let cfg = self.cfg.lock().unwrap().clone();
        if let Some(d) = self.path.parent() {
            let _ = fs::create_dir_all(d);
        }
        let tmp = self.path.with_extension("json.tmp");
        if fs::write(&tmp, serde_json::to_vec_pretty(&cfg).unwrap_or_default()).is_ok() {
            let _ = fs::rename(tmp, &self.path);
        }
    }
}

/// Local calendar fields for a unix time.
#[derive(Clone, Copy, Debug)]
pub struct Local {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub weekday: u8,
}

pub fn local(t: u64) -> Local {
    let tt = t as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointers; localtime_r is the thread-safe variant.
    unsafe { libc::localtime_r(&tt, &mut tm) };
    Local { year: tm.tm_year + 1900, month: (tm.tm_mon + 1) as u8, day: tm.tm_mday as u8, hour: tm.tm_hour as u8, weekday: tm.tm_wday as u8 }
}

/// Unix time of a local date at `hour`:00 (mktime handles day overflow and DST).
fn at(year: i32, month: u8, day: i32, hour: u8) -> u64 {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day;
    tm.tm_hour = hour as i32;
    tm.tm_isdst = -1;
    // SAFETY: tm is fully initialised.
    (unsafe { libc::mktime(&mut tm) }).max(0) as u64
}

/// The latest moment the schedule should have run at, at or before `now`.
pub fn last_due(s: &Schedule, now: u64) -> u64 {
    let l = local(now);
    let today = at(l.year, l.month, l.day as i32, s.hour.min(23));
    match s.every {
        Every::Daily => {
            if today <= now { today } else { at(l.year, l.month, l.day as i32 - 1, s.hour.min(23)) }
        }
        Every::Weekly => {
            let back = (l.weekday as i32 - s.weekday.min(6) as i32).rem_euclid(7);
            let t = at(l.year, l.month, l.day as i32 - back, s.hour.min(23));
            if t <= now { t } else { at(l.year, l.month, l.day as i32 - back - 7, s.hour.min(23)) }
        }
    }
}

/// Due when the last run happened before the latest scheduled moment (this also catches up after the Mac
/// slept or the app was not running). A schedule created after that moment waits for the next one.
pub fn is_due(s: &Schedule, last_run: Option<u64>, created: u64, now: u64) -> bool {
    let due = last_due(s, now);
    due > last_run.unwrap_or(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(y: i32, m: u8, d: i32, h: u8) -> u64 {
        at(y, m, d, h)
    }

    #[test]
    fn daily_runs_once_per_day_and_catches_up() {
        let s = Schedule { every: Every::Daily, hour: 9, weekday: 0, review_ok: false, created: 0 };
        let mon_8 = t(2026, 10, 19, 8);
        let mon_10 = t(2026, 10, 19, 10);
        assert_eq!(last_due(&s, mon_10), t(2026, 10, 19, 9));
        assert_eq!(last_due(&s, mon_8), t(2026, 10, 18, 9), "before 9 the latest due time was yesterday");
        assert!(is_due(&s, Some(t(2026, 10, 18, 9) + 60), 0, mon_10), "ran yesterday, due again today");
        assert!(!is_due(&s, Some(t(2026, 10, 19, 9) + 60), 0, mon_10), "already ran today");
        // the Mac slept through three days: one catch-up run, not three
        assert!(is_due(&s, Some(t(2026, 10, 15, 9)), 0, mon_10));
        // a schedule made after today's time waits until tomorrow
        assert!(!is_due(&s, None, t(2026, 10, 19, 9) + 1, mon_10));
    }

    #[test]
    fn weekly_runs_on_its_weekday() {
        // 2026-10-19 is a Monday (weekday 1)
        assert_eq!(local(t(2026, 10, 19, 12)).weekday, 1);
        let s = Schedule { every: Every::Weekly, hour: 18, weekday: 5, review_ok: false, created: 0 }; // Friday 18:00
        let mon = t(2026, 10, 19, 12);
        assert_eq!(last_due(&s, mon), t(2026, 10, 16, 18), "last Friday");
        let fri_19 = t(2026, 10, 23, 19);
        assert_eq!(last_due(&s, fri_19), t(2026, 10, 23, 18));
        assert!(is_due(&s, Some(t(2026, 10, 16, 18) + 5), 0, fri_19));
        assert!(!is_due(&s, Some(t(2026, 10, 23, 18) + 5), 0, fri_19));
    }

    #[test]
    fn config_round_trips_and_defaults_disk_alert_on() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::load(d.path().join("tracking.json"));
        assert_eq!(s.cfg.lock().unwrap().disk_free_pct, Some(10.0));
        s.cfg.lock().unwrap().schedules.insert("x".into(), Schedule { every: Every::Weekly, hour: 3, weekday: 6, review_ok: false, created: 0 });
        s.save();
        let s2 = Store::load(d.path().join("tracking.json"));
        assert_eq!(s2.cfg.lock().unwrap().schedules["x"].weekday, 6);
    }
}
