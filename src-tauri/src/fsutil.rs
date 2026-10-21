//! Small filesystem helpers shared by the cleanup engine.

use crate::safety::expand_tilde;
use rayon::prelude::*;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measure {
    pub bytes: u64,
    pub files: u64,
    /// Newest modification time found below the path (unix seconds).
    pub mtime: i64,
}

/// Bytes on disk below `path` (symlinks are not followed). Call inside a rayon pool to parallelise.
pub fn measure(path: &Path) -> Measure {
    let Ok(md) = fs::symlink_metadata(path) else { return Measure::default() };
    if md.is_file() {
        return Measure { bytes: md.blocks() * 512, files: 1, mtime: md.mtime() };
    }
    let mut m = Measure { bytes: 0, files: 0, mtime: md.mtime() };
    if !md.is_dir() {
        return m;
    }
    let Ok(rd) = fs::read_dir(path) else { return m };
    let mut dirs = Vec::new();
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            dirs.push(e.path());
        } else if ft.is_file() {
            if let Ok(md) = e.metadata() {
                m.bytes += md.blocks() * 512;
                m.files += 1;
                m.mtime = m.mtime.max(md.mtime());
            }
        }
    }
    for s in dirs.par_iter().map(|d| measure(d)).collect::<Vec<_>>() {
        m.bytes += s.bytes;
        m.files += s.files;
        m.mtime = m.mtime.max(s.mtime);
    }
    m
}

fn opts() -> glob::MatchOptions {
    glob::MatchOptions { case_sensitive: true, require_literal_separator: true, require_literal_leading_dot: false }
}

/// Expand rule globs (with `~`) and drop anything matched by `exclude`. Sorted, de-duplicated.
pub fn expand(patterns: &[String], exclude: &[String], home: &Path) -> Vec<PathBuf> {
    let ex: Vec<glob::Pattern> = exclude.iter().filter_map(|p| glob::Pattern::new(&expand_tilde(p, home)).ok()).collect();
    let mut out = BTreeSet::new();
    for p in patterns {
        let Ok(paths) = glob::glob_with(&expand_tilde(p, home), opts()) else { continue };
        for path in paths.flatten() {
            if !ex.iter().any(|g| g.matches_path_with(&path, opts())) {
                out.insert(path);
            }
        }
    }
    out.into_iter().collect()
}

pub fn list_names(dir: &Path) -> BTreeSet<OsString> {
    fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name()).collect()).unwrap_or_default()
}

/// Free bytes (available to unprivileged users) on the volume holding `path`.
pub fn free_bytes(path: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return 0 };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path and out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    st.f_bavail as u64 * st.f_frsize as u64
}

/// `YYYY-MM-DDTHH:MM:SSZ` (UTC) without a date library.
pub fn iso(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_matches_known_timestamps() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn expand_honours_exclude_and_does_not_cross_separators() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().canonicalize().unwrap();
        for d in ["Library/Caches/a", "Library/Caches/com.apple.x", "Library/Caches/a/deep"] {
            fs::create_dir_all(h.join(d)).unwrap();
        }
        let got = expand(&["~/Library/Caches/*".into()], &["~/Library/Caches/com.apple.*".into()], &h);
        assert_eq!(got, vec![h.join("Library/Caches/a")]);
    }

    #[test]
    fn measure_counts_files_and_ignores_symlinks() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        fs::create_dir_all(p.join("d/e")).unwrap();
        fs::write(p.join("d/a"), vec![1u8; 10_000]).unwrap();
        fs::write(p.join("d/e/b"), vec![1u8; 10_000]).unwrap();
        let out = tempfile::tempdir().unwrap();
        fs::write(out.path().join("big"), vec![1u8; 1_000_000]).unwrap();
        std::os::unix::fs::symlink(out.path(), p.join("d/link")).unwrap();
        let m = measure(&p.join("d"));
        assert_eq!(m.files, 2);
        assert!(m.bytes >= 20_000 && m.bytes < 100_000, "{}", m.bytes);
    }
}
