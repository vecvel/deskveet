//! Per-rule size history: one point per measurement, thinned so it stays small (full detail for 30 days,
//! then one point a day, nothing older than a year).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

pub const DAY: u64 = 86_400;
const FULL_DETAIL: u64 = 30 * DAY;
const KEEP: u64 = 366 * DAY;
/// Measurements closer together than this replace the previous point (the watcher can measure often).
const MIN_GAP: u64 = 3600;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub t: u64,
    pub b: u64,
}

pub fn thin(points: &mut Vec<Point>, now: u64) {
    points.sort_by_key(|p| p.t);
    points.retain(|p| now.saturating_sub(p.t) <= KEEP);
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points.iter().copied() {
        let old = now.saturating_sub(p.t) > FULL_DETAIL;
        match out.last_mut() {
            Some(last) if old && last.t / DAY == p.t / DAY => *last = p,
            Some(last) if !old && p.t.saturating_sub(last.t) < MIN_GAP => *last = p,
            _ => out.push(p),
        }
    }
    *points = out;
}

/// Bytes per day over the last `window` seconds (least squares), when there is enough to tell.
pub fn growth_per_day(points: &[Point], now: u64, window: u64) -> Option<f64> {
    let pts: Vec<&Point> = points.iter().filter(|p| now.saturating_sub(p.t) <= window).collect();
    if pts.len() < 2 || pts.last()?.t - pts.first()?.t < DAY / 2 {
        return None;
    }
    let n = pts.len() as f64;
    let (mx, my) = (pts.iter().map(|p| p.t as f64).sum::<f64>() / n, pts.iter().map(|p| p.b as f64).sum::<f64>() / n);
    let (mut num, mut den) = (0.0, 0.0);
    for p in &pts {
        num += (p.t as f64 - mx) * (p.b as f64 - my);
        den += (p.t as f64 - mx).powi(2);
    }
    (den > 0.0).then(|| num / den * DAY as f64)
}

pub struct History {
    path: PathBuf,
    data: Mutex<HashMap<String, Vec<Point>>>,
}

impl History {
    pub fn load(path: PathBuf) -> History {
        let data = fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        History { path, data: Mutex::new(data) }
    }

    pub fn record(&self, now: u64, sizes: &[(String, u64)]) {
        if sizes.is_empty() {
            return;
        }
        let mut d = self.data.lock().unwrap();
        for (id, b) in sizes {
            let v = d.entry(id.clone()).or_default();
            v.push(Point { t: now, b: *b });
            thin(v, now);
        }
        if let Some(dir) = self.path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.tmp");
        if fs::write(&tmp, serde_json::to_vec(&*d).unwrap_or_default()).is_ok() {
            let _ = fs::rename(tmp, &self.path);
        }
    }

    pub fn get(&self, id: &str) -> Vec<Point> {
        self.data.lock().unwrap().get(id).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinning_keeps_recent_detail_and_one_point_per_old_day() {
        let now = 400 * DAY;
        let mut v: Vec<Point> = (0..400 * 24).map(|h| Point { t: h * 3600, b: h }).collect();
        thin(&mut v, now);
        let recent = v.iter().filter(|p| now - p.t <= FULL_DETAIL).count();
        let old = v.iter().filter(|p| now - p.t > FULL_DETAIL).count();
        assert!(recent >= 30 * 24 - 1, "hourly detail kept for 30 days: {recent}");
        assert!(old <= 337 && old >= 330, "one point per older day within a year: {old}");
        assert!(v.iter().all(|p| now - p.t <= KEEP));
        // points closer than an hour replace each other
        let mut w = vec![Point { t: now - 100, b: 1 }, Point { t: now - 50, b: 2 }, Point { t: now, b: 3 }];
        thin(&mut w, now);
        assert_eq!(w, vec![Point { t: now, b: 3 }]);
    }

    #[test]
    fn growth_is_bytes_per_day() {
        let now = 100 * DAY;
        let v: Vec<Point> = (0..10).map(|d| Point { t: now - (9 - d) * DAY, b: 1_000 + d * 500 }).collect();
        let g = growth_per_day(&v, now, 30 * DAY).unwrap();
        assert!((g - 500.0).abs() < 1.0, "{g}");
        assert!(growth_per_day(&v[..1], now, 30 * DAY).is_none());
    }

    #[test]
    fn records_persist_across_loads() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("h/history.json");
        let h = History::load(p.clone());
        h.record(10 * DAY, &[("a".into(), 5)]);
        h.record(12 * DAY, &[("a".into(), 7)]);
        assert_eq!(History::load(p).get("a").len(), 2);
    }
}
