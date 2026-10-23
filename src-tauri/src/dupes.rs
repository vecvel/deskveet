//! Duplicate files: files below a folder with identical contents. This only finds them; removing one is an
//! explicit pick that goes through the normal preview and the Trash, and the last copy is never offered.
//!
//! Files are grouped by size, then by a hash of their first and last 64 KiB, then by a 128-bit hash of the
//! whole file (two SipHash passes with keys chosen at random for each search). Hard links are one file, and
//! files that are only in iCloud are skipped rather than downloaded.

use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

/// Folders that are one document or one app to macOS, or that a tool owns. Their insides are never compared.
const PACKAGES: &[&str] = &[
    "app", "appex", "bundle", "framework", "plugin", "kext", "xpc", "photoslibrary", "photolibrary", "aplibrary",
    "musiclibrary", "tvlibrary", "imovielibrary", "fcpbundle", "logicx", "band", "xcarchive", "xcodeproj",
    "xcworkspace", "playground", "pkg", "mpkg", "sparsebundle", "dmg", "docarchive", "rtfd", "pages", "numbers", "key",
];
const SKIP_DIRS: &[&str] = &[".git", ".hg", ".svn", "node_modules", ".Trash", ".Spotlight-V100", ".fseventsd", ".DocumentRevisions-V100"];
const EDGE: u64 = 64 * 1024;

/// Live numbers for the progress line.
#[derive(Default)]
pub struct Counters {
    /// Files of at least the minimum size seen so far.
    pub files: AtomicU64,
    /// Bytes read so far while comparing, and the bytes there are to read.
    pub read: AtomicU64,
    pub to_read: AtomicU64,
    pub comparing: AtomicBool,
    pub current: Mutex<String>,
}

#[derive(Clone, Debug)]
pub struct Copy {
    pub path: PathBuf,
    pub mtime: i64,
}

#[derive(Clone, Debug)]
pub struct Group {
    /// Size of each copy in bytes.
    pub size: u64,
    pub copies: Vec<Copy>,
}

impl Group {
    /// Bytes taken by every copy but one.
    pub fn extra(&self) -> u64 {
        self.size * (self.copies.len() as u64 - 1)
    }
}

struct Ent {
    path: PathBuf,
    size: u64,
    dev: u64,
    ino: u64,
    mtime: i64,
}

/// Every group of two or more identical files of at least `min_size` bytes below `root`, on its volume only,
/// largest saving first. `skip` leaves out whole folders (the caller's protected places). `None` when cancelled.
pub fn find(root: &Path, min_size: u64, skip: &(dyn Fn(&Path) -> bool + Sync), cancel: &AtomicBool, n: &Counters) -> Option<Vec<Group>> {
    let dev = fs::symlink_metadata(root).ok()?.dev();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(6).thread_name(|i| format!("dupes-{i}")).build().ok()?;
    pool.install(|| {
        let found = Mutex::new(Vec::new());
        list(root, dev, min_size.max(1), skip, cancel, n, &found);
        if cancel.load(Relaxed) {
            return None;
        }
        let mut seen = HashSet::new();
        let mut by_size: HashMap<u64, Vec<Ent>> = HashMap::new();
        for e in found.into_inner().unwrap() {
            if seen.insert((e.dev, e.ino)) {
                by_size.entry(e.size).or_default().push(e); // a hard link is the same file, not a copy
            }
        }
        let sets: Vec<Vec<Ent>> = by_size.into_values().filter(|v| v.len() > 1).collect();
        n.comparing.store(true, Relaxed);
        n.to_read.store(sets.iter().flatten().map(|e| e.size.min(2 * EDGE)).sum(), Relaxed);
        let keys = (RandomState::new(), RandomState::new());
        let sets = split(sets, |e| edges(&e.path, e.size, &keys, cancel, n), cancel)?;
        n.read.store(0, Relaxed);
        n.to_read.store(sets.iter().flatten().map(|e| e.size).sum(), Relaxed);
        let sets = split(sets, |e| whole(&e.path, &keys, cancel, n), cancel)?;
        let mut groups: Vec<Group> = sets
            .into_iter()
            .map(|v| {
                let size = v[0].size;
                let mut copies: Vec<Copy> = v.into_iter().map(|e| Copy { path: e.path, mtime: e.mtime }).collect();
                copies.sort_by(|a, b| a.mtime.cmp(&b.mtime).then_with(|| a.path.cmp(&b.path)));
                Group { size, copies }
            })
            .collect();
        groups.sort_by(|a, b| b.extra().cmp(&a.extra()).then_with(|| a.copies[0].path.cmp(&b.copies[0].path)));
        Some(groups)
    })
}

/// Split each set by `key`, keeping only sub-sets with two or more files. Unreadable files drop out.
fn split<K: Eq + std::hash::Hash + Send>(sets: Vec<Vec<Ent>>, key: impl Fn(&Ent) -> Option<K> + Sync, cancel: &AtomicBool) -> Option<Vec<Vec<Ent>>> {
    let out: Vec<Vec<Ent>> = sets
        .into_par_iter()
        .flat_map_iter(|set| {
            let keyed: Vec<(K, Ent)> = set.into_iter().filter_map(|e| Some((key(&e)?, e))).collect();
            let mut by: HashMap<K, Vec<Ent>> = HashMap::new();
            for (k, e) in keyed {
                by.entry(k).or_default().push(e);
            }
            by.into_values().filter(|v| v.len() > 1)
        })
        .collect();
    (!cancel.load(Relaxed)).then_some(out)
}

fn list(dir: &Path, dev: u64, min: u64, skip: &(dyn Fn(&Path) -> bool + Sync), cancel: &AtomicBool, n: &Counters, out: &Mutex<Vec<Ent>>) {
    if cancel.load(Relaxed) || skip(dir) {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    if let Ok(mut c) = n.current.try_lock() {
        *c = dir.to_string_lossy().into_owned();
    }
    let mut subdirs = Vec::new();
    let mut mine = Vec::new();
    for entry in rd.flatten() {
        let Ok(m) = entry.metadata() else { continue }; // does not follow symlinks
        if m.dev() != dev {
            continue; // another volume mounted inside this one
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if m.is_dir() {
            let ext = Path::new(&*name).extension().map(|e| e.to_string_lossy().to_lowercase());
            if !SKIP_DIRS.contains(&&*name) && !ext.is_some_and(|e| PACKAGES.contains(&e.as_str())) {
                subdirs.push(entry.path());
            }
        } else if m.is_file() && m.len() >= min && !dataless(&m) && name != ".DS_Store" {
            mine.push(Ent { path: entry.path(), size: m.len(), dev: m.dev(), ino: m.ino(), mtime: m.mtime() });
        }
    }
    if !mine.is_empty() {
        n.files.fetch_add(mine.len() as u64, Relaxed);
        out.lock().unwrap().extend(mine);
    }
    subdirs.par_iter().for_each(|d| list(d, dev, min, skip, cancel, n, out));
}

/// A file whose contents are only in iCloud: reading it would download it.
#[cfg(target_os = "macos")]
fn dataless(m: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    const SF_DATALESS: u32 = 0x4000_0000;
    m.st_flags() & SF_DATALESS != 0
}
#[cfg(not(target_os = "macos"))]
fn dataless(_: &fs::Metadata) -> bool {
    false
}

type Keys = (RandomState, RandomState);

fn edges(p: &Path, size: u64, keys: &Keys, cancel: &AtomicBool, n: &Counters) -> Option<(u64, u64)> {
    let mut f = File::open(p).ok()?;
    let (mut a, mut b) = (keys.0.build_hasher(), keys.1.build_hasher());
    feed(&mut f, EDGE.min(size), &mut a, &mut b, cancel, n).ok()?;
    if size > 2 * EDGE {
        f.seek(SeekFrom::Start(size - EDGE)).ok()?;
        feed(&mut f, EDGE, &mut a, &mut b, cancel, n).ok()?;
    } else if size > EDGE {
        feed(&mut f, size - EDGE, &mut a, &mut b, cancel, n).ok()?;
    }
    Some((a.finish(), b.finish()))
}

fn whole(p: &Path, keys: &Keys, cancel: &AtomicBool, n: &Counters) -> Option<(u64, u64)> {
    let mut f = File::open(p).ok()?;
    let (mut a, mut b) = (keys.0.build_hasher(), keys.1.build_hasher());
    feed(&mut f, u64::MAX, &mut a, &mut b, cancel, n).ok()?;
    Some((a.finish(), b.finish()))
}

/// Hash up to `len` bytes (to the end with `u64::MAX`), stopping early when cancelled.
fn feed(f: &mut File, len: u64, a: &mut impl Hasher, b: &mut impl Hasher, cancel: &AtomicBool, n: &Counters) -> io::Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    let mut left = len;
    while left > 0 {
        if cancel.load(Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let got = f.read(&mut buf[..want])?;
        if got == 0 {
            break;
        }
        a.write(&buf[..got]);
        b.write(&buf[..got]);
        n.read.fetch_add(got as u64, Relaxed);
        left -= got as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(p: &Path, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn run(root: &Path, min: u64, skip: &(dyn Fn(&Path) -> bool + Sync)) -> Vec<Group> {
        find(root, min, skip, &AtomicBool::new(false), &Counters::default()).unwrap()
    }

    #[test]
    fn finds_identical_files_only() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let mut middle = big.clone();
        middle[150_000] ^= 1; // same size, same first and last 64 KiB, different in the middle
        put(&r.join("a/photo.jpg"), &big);
        put(&r.join("b/photo copy.jpg"), &big);
        put(&r.join("c/again.jpg"), &big);
        put(&r.join("d/almost.jpg"), &middle);
        put(&r.join("e/small1.txt"), b"hello");
        put(&r.join("e/small2.txt"), b"hello");
        put(&r.join("f/other.txt"), b"world");
        fs::hard_link(r.join("a/photo.jpg"), r.join("a/link.jpg")).unwrap();
        std::os::unix::fs::symlink(r.join("a/photo.jpg"), r.join("a/sym.jpg")).unwrap();

        let g = run(r, 1, &|_| false);
        assert_eq!(g.len(), 2, "{g:?}");
        assert_eq!(g[0].size, 300_000);
        let names: HashSet<String> = g[0].copies.iter().map(|c| c.path.strip_prefix(r).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 3, "hard link counted once, symlink and near-copy left out: {names:?}");
        assert!(names.contains("b/photo copy.jpg") && names.contains("c/again.jpg"));
        assert!(!names.iter().any(|n| n.contains("almost") || n.contains("sym")));
        assert_eq!(g[0].extra(), 600_000);
        assert_eq!(g[1].copies.len(), 2);

        assert_eq!(run(r, 1000, &|_| false).len(), 1, "minimum size");
    }

    #[test]
    fn skips_packages_tool_folders_and_skipped_places() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        put(&r.join("keep/one.bin"), b"same bytes");
        put(&r.join("Some.app/Contents/one.bin"), b"same bytes");
        put(&r.join("proj/node_modules/x/one.bin"), b"same bytes");
        put(&r.join("proj/.git/objects/one.bin"), b"same bytes");
        put(&r.join("Library/one.bin"), b"same bytes");
        let lib = r.join("Library");
        assert!(run(r, 1, &|p| p == lib).is_empty(), "only one copy outside packages, tool folders and skipped places");
        assert_eq!(run(r, 1, &|_| false).len(), 1, "the copy in Library counts when it is not skipped");
        put(&r.join("keep/two.bin"), b"same bytes");
        let g = run(r, 1, &|p| p == lib);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].copies.len(), 2);
        assert!(g[0].copies.iter().all(|c| c.path.starts_with(r.join("keep"))));
    }

    #[test]
    fn oldest_copy_comes_first_and_cancel_stops() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        put(&r.join("new.txt"), b"abc");
        put(&r.join("old.txt"), b"abc");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400 * 30);
        File::options().write(true).open(r.join("old.txt")).unwrap().set_modified(old).unwrap();
        let g = run(r, 1, &|_| false);
        assert_eq!(g[0].copies[0].path, r.join("old.txt"));
        assert!(find(r, 1, &|_| false, &AtomicBool::new(true), &Counters::default()).is_none());
    }
}
