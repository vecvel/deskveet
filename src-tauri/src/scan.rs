//! Parallel disk scanner.
//!
//! The walk aggregates sizes bottom-up and keeps only what the UI needs, so memory stays small
//! even for millions of files: directories below `keep_min` are folded into their parent, and each
//! directory lists only its largest files. A compact per-directory index (mtime + own totals) is
//! cached next to the tree so a re-scan only re-stats files in directories whose mtime changed.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const KEEP_MIN: u64 = 1 << 20; // 1 MiB
pub const TOP_FILES: usize = 24;
/// Cached directory data older than this is ignored (file *contents* can change without bumping a dir mtime).
pub const MAX_CACHE_AGE_SECS: u64 = 24 * 3600;
const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEnt {
    pub name: Box<str>,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Node {
    pub name: Box<str>,
    /// Bytes on disk (st_blocks * 512), recursive.
    pub size: u64,
    /// Files below this directory, recursive.
    pub files: u64,
    pub mtime: i64,
    pub kids: Vec<Node>,
    pub top: Vec<FileEnt>,
    /// Folded sub-directories: (count, recursive bytes).
    pub small_dirs: (u32, u64),
    /// Files in this directory that are not in `top`: (count, bytes).
    pub small_files: (u32, u64),
}

#[derive(Clone, Copy)]
pub struct Opts {
    pub keep_min: u64,
    pub top_n: usize,
    pub threads: usize,
}

impl Default for Opts {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        Opts { keep_min: KEEP_MIN, top_n: TOP_FILES, threads: (cpus * 2).clamp(2, 12) }
    }
}

// ───────────────────────────── index (incremental re-scan) ─────────────────────────────
//
// One fixed-size record per directory that has files: path hash, mtime (ns), own bytes, own file count,
// plus its top files when it has any. The previous index is held as a sorted array (32 bytes/dir)
// and the new one is streamed to disk while scanning, so neither costs much RAM.

const IDX_MAGIC: &[u8; 4] = b"DKV2";

#[derive(Clone, Copy, Debug)]
struct PrevRec {
    hash: u64,
    mtime: i64,
    bytes: u64,
    files: u32,
}

#[derive(Default, Debug)]
pub struct Prev {
    recs: Vec<PrevRec>,
    tops: HashMap<u64, Vec<FileEnt>>,
}

impl Prev {
    pub fn len(&self) -> usize {
        self.recs.len()
    }

    fn get(&self, hash: u64) -> Option<(&PrevRec, &[FileEnt])> {
        let i = self.recs.binary_search_by_key(&hash, |r| r.hash).ok()?;
        Some((&self.recs[i], self.tops.get(&hash).map(|v| v.as_slice()).unwrap_or(&[])))
    }
}

fn fnv(p: &Path) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in p.as_os_str().as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Streams index records to `<path>.tmp` and renames it into place on success.
pub struct IdxWriter {
    w: Mutex<Option<io::BufWriter<fs::File>>>,
    count: AtomicU64,
    path: PathBuf,
}

impl IdxWriter {
    pub fn create(path: &Path) -> io::Result<Self> {
        if let Some(d) = path.parent() {
            fs::create_dir_all(d)?;
        }
        let mut w = io::BufWriter::with_capacity(1 << 20, fs::File::create(tmp_of(path))?);
        w.write_all(IDX_MAGIC)?;
        w.write_all(&0u64.to_le_bytes())?; // record count, patched in finish()
        Ok(IdxWriter { w: Mutex::new(Some(w)), count: 0.into(), path: path.to_path_buf() })
    }

    fn push(&self, hash: u64, mtime: i64, bytes: u64, files: u32, top: &[FileEnt]) {
        let mut g = self.w.lock().unwrap();
        let Some(w) = g.as_mut() else { return };
        let mut rec = Vec::with_capacity(33 + top.len() * 40);
        rec.extend_from_slice(&hash.to_le_bytes());
        rec.extend_from_slice(&mtime.to_le_bytes());
        rec.extend_from_slice(&bytes.to_le_bytes());
        rec.extend_from_slice(&files.to_le_bytes());
        rec.push(top.len().min(255) as u8);
        for f in top.iter().take(255) {
            let n = f.name.as_bytes();
            let n = &n[..n.len().min(u16::MAX as usize)];
            rec.extend_from_slice(&f.size.to_le_bytes());
            rec.extend_from_slice(&f.mtime.to_le_bytes());
            rec.extend_from_slice(&(n.len() as u16).to_le_bytes());
            rec.extend_from_slice(n);
        }
        if w.write_all(&rec).is_ok() {
            self.count.fetch_add(1, Relaxed);
        }
    }

    /// `keep = false` discards the partial file (cancelled scan).
    pub fn finish(&self, keep: bool) -> io::Result<()> {
        let Some(mut w) = self.w.lock().unwrap().take() else { return Ok(()) };
        if !keep {
            drop(w);
            let _ = fs::remove_file(tmp_of(&self.path));
            return Ok(());
        }
        use io::{Seek, SeekFrom};
        w.flush()?;
        let f = w.get_mut();
        f.seek(SeekFrom::Start(4))?;
        f.write_all(&self.count.load(Relaxed).to_le_bytes())?;
        drop(w);
        fs::rename(tmp_of(&self.path), &self.path)
    }
}

fn tmp_of(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

pub fn read_index(path: &Path) -> Option<Prev> {
    use io::Read;
    let mut r = io::BufReader::with_capacity(1 << 20, fs::File::open(path).ok()?);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic).ok()?;
    if &magic != IDX_MAGIC {
        return None;
    }
    let mut b8 = [0u8; 8];
    let mut b4 = [0u8; 4];
    let mut b2 = [0u8; 2];
    let mut b1 = [0u8; 1];
    r.read_exact(&mut b8).ok()?;
    let count = u64::from_le_bytes(b8) as usize;
    let mut prev = Prev { recs: Vec::with_capacity(count.min(8_000_000)), tops: HashMap::new() };
    for _ in 0..count {
        r.read_exact(&mut b8).ok()?;
        let hash = u64::from_le_bytes(b8);
        r.read_exact(&mut b8).ok()?;
        let mtime = i64::from_le_bytes(b8);
        r.read_exact(&mut b8).ok()?;
        let bytes = u64::from_le_bytes(b8);
        r.read_exact(&mut b4).ok()?;
        let files = u32::from_le_bytes(b4);
        r.read_exact(&mut b1).ok()?;
        let nt = b1[0] as usize;
        if nt > 0 {
            let mut top = Vec::with_capacity(nt);
            for _ in 0..nt {
                r.read_exact(&mut b8).ok()?;
                let size = u64::from_le_bytes(b8);
                r.read_exact(&mut b8).ok()?;
                let fm = i64::from_le_bytes(b8);
                r.read_exact(&mut b2).ok()?;
                let mut name = vec![0u8; u16::from_le_bytes(b2) as usize];
                r.read_exact(&mut name).ok()?;
                top.push(FileEnt { name: String::from_utf8_lossy(&name).into_owned().into_boxed_str(), size, mtime: fm });
            }
            prev.tops.insert(hash, top);
        }
        prev.recs.push(PrevRec { hash, mtime, bytes, files });
    }
    prev.recs.sort_unstable_by_key(|r| r.hash);
    Some(prev)
}

// ───────────────────────────── scanning ─────────────────────────────

pub struct Ctx {
    pub cancel: Arc<AtomicBool>,
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub dirs: AtomicU64,
    pub reused: AtomicU64,
    pub denied_n: AtomicU64,
    /// EPERM (macOS privacy protection) — the case Full Disk Access fixes.
    pub protected_n: AtomicU64,
    pub denied: Mutex<Vec<String>>,
    pub current: Mutex<String>,
    pub idx: Option<IdxWriter>,
    prev: Option<Prev>,
    root_dev: u64,
    opts: Opts,
}

impl Ctx {
    pub fn new(cancel: Arc<AtomicBool>, prev: Option<Prev>, root_dev: u64, opts: Opts, idx: Option<IdxWriter>) -> Self {
        Ctx {
            cancel,
            files: 0.into(),
            bytes: 0.into(),
            dirs: 0.into(),
            reused: 0.into(),
            denied_n: 0.into(),
            protected_n: 0.into(),
            denied: Mutex::new(Vec::new()),
            current: Mutex::new(String::new()),
            idx,
            prev,
            root_dev,
            opts,
        }
    }

    fn deny(&self, p: &Path, e: &io::Error) {
        if !matches!(e.kind(), io::ErrorKind::PermissionDenied) && e.raw_os_error() != Some(1) {
            return; // vanished while scanning, etc.
        }
        self.denied_n.fetch_add(1, Relaxed);
        if e.raw_os_error() == Some(1) {
            self.protected_n.fetch_add(1, Relaxed);
        }
        if let Ok(mut d) = self.denied.lock() {
            if d.len() < 200 {
                d.push(p.to_string_lossy().into_owned());
            }
        }
    }
}

fn scan_dir(path: &Path, name: Box<str>, ctx: &Ctx) -> Option<Node> {
    if ctx.cancel.load(Relaxed) {
        return None;
    }
    let md = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            ctx.deny(path, &e);
            return None;
        }
    };
    if md.dev() != ctx.root_dev {
        return None; // never cross into another volume
    }
    // Nanosecond precision for the change check: a directory modified right after a scan must not look unchanged.
    let mtime_ns = md.mtime().saturating_mul(1_000_000_000).saturating_add(md.mtime_nsec());
    let mut node = Node { name, mtime: md.mtime(), ..Default::default() };
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(e) => {
            ctx.deny(path, &e);
            return Some(node);
        }
    };

    let n = ctx.dirs.fetch_add(1, Relaxed);
    if n % 64 == 0 {
        if let Ok(mut c) = ctx.current.try_lock() {
            *c = path.to_string_lossy().into_owned();
        }
    }

    let hash = fnv(path);
    let reuse = ctx.prev.as_ref().and_then(|p| p.get(hash)).filter(|(r, _)| r.mtime == mtime_ns);

    let mut subdirs: Vec<(Box<str>, PathBuf)> = Vec::new();
    let mut own_bytes = 0u64;
    let mut own_files = 0u64;
    let mut top: Vec<FileEnt> = Vec::new();
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            subdirs.push((entry.file_name().to_string_lossy().into_owned().into_boxed_str(), entry.path()));
        } else if ft.is_file() && reuse.is_none() {
            let Ok(m) = entry.metadata() else { continue };
            let size = m.blocks() * 512;
            own_bytes += size;
            own_files += 1;
            if size >= ctx.opts.keep_min {
                let ent = FileEnt { name: entry.file_name().to_string_lossy().into_owned().into_boxed_str(), size, mtime: m.mtime() };
                if top.len() < ctx.opts.top_n {
                    top.push(ent);
                } else if let Some((i, min)) = top.iter().enumerate().min_by_key(|(_, f)| f.size) {
                    if size > min.size {
                        top[i] = ent;
                    }
                }
            }
        }
    }
    if let Some((r, t)) = reuse {
        own_bytes = r.bytes;
        own_files = r.files as u64;
        top = t.to_vec();
        ctx.reused.fetch_add(1, Relaxed);
    }
    ctx.files.fetch_add(own_files, Relaxed);
    ctx.bytes.fetch_add(own_bytes, Relaxed);
    if own_files > 0 {
        if let Some(w) = &ctx.idx {
            w.push(hash, mtime_ns, own_bytes, own_files.min(u32::MAX as u64) as u32, &top);
        }
    }

    top.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    let top_bytes: u64 = top.iter().map(|f| f.size).sum();
    node.small_files = ((own_files - top.len() as u64) as u32, own_bytes - top_bytes);
    node.size = own_bytes;
    node.files = own_files;
    node.top = top;

    let kids: Vec<Node> = subdirs.into_par_iter().filter_map(|(n, p)| scan_dir(&p, n, ctx)).collect();
    for k in kids {
        node.size += k.size;
        node.files += k.files;
        if k.size >= ctx.opts.keep_min {
            node.kids.push(k);
        } else {
            node.small_dirs.0 += 1;
            node.small_dirs.1 += k.size;
        }
    }
    node.kids.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    Some(node)
}

/// Scan `root` on a dedicated thread pool (freed when the scan ends). `None` when cancelled or unreadable.
/// The caller finishes the index writer (keep on success, discard otherwise).
pub fn run(root: &Path, ctx: &Ctx) -> Option<Node> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(ctx.opts.threads)
        .thread_name(|i| format!("scan-{i}"))
        .build()
        .ok()?;
    let name: Box<str> = root.to_string_lossy().into_owned().into_boxed_str();
    let tree = pool.install(|| scan_dir(root, name, ctx))?;
    (!ctx.cancel.load(Relaxed)).then_some(tree)
}

pub fn dev_of(p: &Path) -> io::Result<u64> {
    Ok(fs::symlink_metadata(p)?.dev())
}

// ───────────────────────────── cache ─────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Meta {
    pub v: u32,
    pub root: String,
    pub scanned_at: u64,
    pub duration_ms: u64,
    pub files: u64,
    pub bytes: u64,
    pub dirs: u64,
    pub denied_n: u64,
    pub protected_n: u64,
    pub denied: Vec<String>,
}

#[derive(Serialize)]
struct CachedTree<'a> {
    meta: &'a Meta,
    tree: &'a Node,
}

#[derive(Deserialize)]
struct CachedTreeOwned {
    meta: Meta,
    tree: Node,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn tree_path(dir: &Path, root: &Path) -> PathBuf {
    dir.join(format!("scan-{:016x}.json", fnv(root)))
}

pub fn idx_path(dir: &Path, root: &Path) -> PathBuf {
    dir.join(format!("scan-{:016x}.idx", fnv(root)))
}

pub fn save_tree(dir: &Path, root: &Path, meta: &Meta, tree: &Node) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tp = tree_path(dir, root);
    let tmp = tmp_of(&tp);
    serde_json::to_writer(io::BufWriter::new(fs::File::create(&tmp)?), &CachedTree { meta, tree })?;
    fs::rename(tmp, tp)
}

pub fn load_cache(dir: &Path, root: &Path) -> Option<(Meta, Node)> {
    let c: CachedTreeOwned = serde_json::from_reader(io::BufReader::new(fs::File::open(tree_path(dir, root)).ok()?)).ok()?;
    (c.meta.v == CACHE_VERSION && Path::new(&c.meta.root) == root).then_some((c.meta, c.tree))
}

/// The directory index for incremental re-scans, if it was written less than a day before `now`.
pub fn load_prev_index(dir: &Path, root: &Path, now: u64) -> Option<Prev> {
    let path = idx_path(dir, root);
    let written = fs::metadata(&path).ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    if now.saturating_sub(written) > MAX_CACHE_AGE_SECS {
        return None;
    }
    read_index(&path)
}

pub fn clear_cache(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("scan-") && fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    n
}

pub fn meta_for(root: &Path, ctx: &Ctx, tree: &Node, duration_ms: u64) -> Meta {
    Meta {
        v: CACHE_VERSION,
        root: root.to_string_lossy().into_owned(),
        scanned_at: now_secs(),
        duration_ms,
        files: tree.files,
        bytes: tree.size,
        dirs: ctx.dirs.load(Relaxed),
        denied_n: ctx.denied_n.load(Relaxed),
        protected_n: ctx.protected_n.load(Relaxed),
        denied: ctx.denied.lock().map(|d| d.clone()).unwrap_or_default(),
    }
}

// ───────────────────────────── browsing the tree ─────────────────────────────

#[derive(Serialize, Debug, PartialEq)]
pub struct Row {
    pub name: String,
    pub path: String,
    /// dir | file | small_dirs | small_files
    pub kind: &'static str,
    pub size: u64,
    pub files: u64,
    pub mtime: i64,
}

/// Find the node for `target` below `root`; both must be absolute.
pub fn find<'a>(tree: &'a Node, root: &Path, target: &Path) -> Option<&'a Node> {
    let rel = target.strip_prefix(root).ok()?;
    let mut cur = tree;
    for c in rel.components() {
        let name = c.as_os_str().to_string_lossy();
        cur = cur.kids.iter().find(|k| *k.name == *name)?;
    }
    Some(cur)
}

/// Every file below `node` of at least `min_bytes` that has not changed since `changed_before` (unix
/// seconds), largest first, at most `limit`. Uses the scan's per-folder list of largest files, which a file
/// this large is always part of.
pub fn large_files(node: &Node, base: &Path, min_bytes: u64, changed_before: i64, limit: usize) -> Vec<(PathBuf, u64, i64)> {
    fn walk(n: &Node, dir: &Path, min: u64, before: i64, out: &mut Vec<(PathBuf, u64, i64)>) {
        if n.size < min {
            return; // nothing below can be this large
        }
        for f in n.top.iter().filter(|f| f.size >= min && f.mtime <= before) {
            out.push((dir.join(&*f.name), f.size, f.mtime));
        }
        for k in &n.kids {
            walk(k, &dir.join(&*k.name), min, before, out);
        }
    }
    let mut out = Vec::new();
    walk(node, base, min_bytes, changed_before, &mut out);
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out.truncate(limit);
    out
}

pub fn rows(node: &Node, base: &Path) -> Vec<Row> {
    let mut out = Vec::with_capacity(node.kids.len() + node.top.len() + 2);
    for k in &node.kids {
        out.push(Row { name: k.name.to_string(), path: base.join(&*k.name).to_string_lossy().into_owned(), kind: "dir", size: k.size, files: k.files, mtime: k.mtime });
    }
    for f in &node.top {
        out.push(Row { name: f.name.to_string(), path: base.join(&*f.name).to_string_lossy().into_owned(), kind: "file", size: f.size, files: 1, mtime: f.mtime });
    }
    if node.small_dirs.0 > 0 {
        out.push(Row { name: format!("{} smaller folders", node.small_dirs.0), path: base.to_string_lossy().into_owned(), kind: "small_dirs", size: node.small_dirs.1, files: 0, mtime: 0 });
    }
    if node.small_files.0 > 0 {
        out.push(Row { name: format!("{} smaller files", node.small_files.0), path: base.to_string_lossy().into_owned(), kind: "small_files", size: node.small_files.1, files: node.small_files.0 as u64, mtime: 0 });
    }
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_files_finds_big_old_files_anywhere_below_a_folder() {
        let f = |name: &str, mb: u64, mtime: i64| FileEnt { name: name.into(), size: mb * 1_000_000, mtime };
        let movies = Node { name: "Movies".into(), size: 3_000_000_000, top: vec![f("old.mov", 2_000, 100), f("new.mov", 900, 5_000)], ..Default::default() };
        let deep = Node { name: "b".into(), size: 600_000_000, top: vec![f("disk.img", 600, 50)], ..Default::default() };
        let a = Node { name: "a".into(), size: 600_000_000, kids: vec![deep], ..Default::default() };
        let small = Node { name: "tiny".into(), size: 10_000_000, top: vec![f("x", 10, 1)], ..Default::default() };
        let root = Node { name: "home".into(), size: 4_000_000_000, kids: vec![movies, a, small], top: vec![f("big.zip", 500, 10)], ..Default::default() };
        let got = large_files(&root, Path::new("/h"), 500_000_000, 1_000, 10);
        let names: Vec<String> = got.iter().map(|(p, _, _)| p.to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["/h/Movies/old.mov", "/h/a/b/disk.img", "/h/big.zip"], "largest first; new.mov changed too recently");
        assert_eq!(large_files(&root, Path::new("/h"), 500_000_000, 1_000, 1).len(), 1, "limit");
    }
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn opts() -> Opts {
        Opts { keep_min: 64 * 1024, top_n: 3, threads: 4 }
    }
    fn write(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![7u8; bytes]).unwrap();
    }
    fn scan_with(root: &Path, prev: Option<Prev>, idx: Option<&Path>) -> (Option<Node>, Ctx) {
        let w = idx.map(|p| IdxWriter::create(p).unwrap());
        let ctx = Ctx::new(Arc::new(AtomicBool::new(false)), prev, dev_of(root).unwrap(), opts(), w);
        let tree = run(root, &ctx);
        if let Some(w) = &ctx.idx {
            w.finish(tree.is_some()).unwrap();
        }
        (tree, ctx)
    }
    fn scan(root: &Path) -> (Option<Node>, Ctx) {
        scan_with(root, None, None)
    }
    fn find_kid<'a>(n: &'a Node, name: &str) -> &'a Node {
        n.kids.iter().find(|k| &*k.name == name).unwrap_or_else(|| panic!("no kid {name}"))
    }

    #[test]
    fn sizes_aggregate_and_small_dirs_fold() {
        let t = tempdir().unwrap();
        write(&t.path().join("big/a.bin"), 400_000);
        write(&t.path().join("big/deep/b.bin"), 300_000);
        write(&t.path().join("tiny/c.txt"), 100);
        let tree = scan(t.path()).0.unwrap();
        let big = find_kid(&tree, "big");
        // st_blocks rounds up to the filesystem block size
        assert!(big.size >= 700_000 && big.size < 700_000 + 64 * 1024, "{}", big.size);
        assert_eq!(big.files, 2);
        assert_eq!(tree.files, 3);
        assert!(tree.size >= big.size);
        // `tiny` is below keep_min: folded, but its bytes still count in the parent total.
        assert!(tree.kids.iter().all(|k| &*k.name != "tiny"));
        assert_eq!(tree.small_dirs.0, 1);
        assert!(tree.size >= big.size + tree.small_dirs.1);
    }

    #[test]
    fn largest_files_are_kept_sorted() {
        let t = tempdir().unwrap();
        for (i, sz) in [100_000usize, 500_000, 300_000, 200_000, 10].iter().enumerate() {
            write(&t.path().join(format!("f{i}.bin")), *sz);
        }
        let tree = scan(t.path()).0.unwrap();
        assert_eq!(tree.top.len(), 3);
        assert!(tree.top[0].size >= 500_000 && tree.top[0].size >= tree.top[1].size && tree.top[1].size >= tree.top[2].size);
        assert_eq!(&*tree.top[0].name, "f1.bin");
        assert_eq!(tree.small_files.0, 2);
    }

    #[test]
    fn symlinks_are_not_followed() {
        let t = tempdir().unwrap();
        let outside = tempdir().unwrap();
        write(&outside.path().join("huge.bin"), 1_000_000);
        write(&t.path().join("real.bin"), 100_000);
        std::os::unix::fs::symlink(outside.path(), t.path().join("link")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("huge.bin"), t.path().join("flink")).unwrap();
        let tree = scan(t.path()).0.unwrap();
        assert_eq!(tree.files, 1);
        assert!(tree.size < 200_000);
    }

    #[test]
    fn permission_denied_is_reported_not_fatal() {
        let t = tempdir().unwrap();
        write(&t.path().join("ok.bin"), 100_000);
        let locked = t.path().join("locked");
        fs::create_dir(&locked).unwrap();
        write(&locked.join("x.bin"), 10);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let (tree, ctx) = scan(t.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(tree.is_some());
        assert_eq!(ctx.denied_n.load(Relaxed), 1);
        assert!(ctx.denied.lock().unwrap()[0].ends_with("locked"));
    }

    #[test]
    fn cancel_stops_the_scan_and_discards_the_partial_index() {
        let t = tempdir().unwrap();
        let out = tempdir().unwrap();
        write(&t.path().join("a/b.bin"), 100_000);
        let idx = out.path().join("x.idx");
        let w = IdxWriter::create(&idx).unwrap();
        let ctx = Ctx::new(Arc::new(AtomicBool::new(true)), None, dev_of(t.path()).unwrap(), opts(), Some(w));
        let tree = run(t.path(), &ctx);
        ctx.idx.as_ref().unwrap().finish(tree.is_some()).unwrap();
        assert!(tree.is_none());
        assert!(!idx.exists() && !tmp_of(&idx).exists());
    }

    #[test]
    fn incremental_rescan_reuses_unchanged_dirs_and_sees_changes() {
        let t = tempdir().unwrap();
        let out = tempdir().unwrap();
        write(&t.path().join("a/one.bin"), 200_000);
        write(&t.path().join("b/two.bin"), 200_000);
        let idx = out.path().join("i.idx");
        let first = scan_with(t.path(), None, Some(&idx)).0.unwrap();

        let prev = read_index(&idx).unwrap();
        assert_eq!(prev.len(), 2);
        let idx2 = out.path().join("i2.idx");
        let (t2, ctx2) = scan_with(t.path(), Some(prev), Some(&idx2));
        assert_eq!(t2.unwrap().size, first.size);
        assert!(ctx2.reused.load(Relaxed) >= 2, "reused {}", ctx2.reused.load(Relaxed));

        // Adding a file bumps the directory mtime, so only that directory is re-statted.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&t.path().join("a/three.bin"), 300_000);
        let (t3, ctx3) = scan_with(t.path(), read_index(&idx2), None);
        let t3 = t3.unwrap();
        assert!(t3.size >= first.size + 300_000);
        assert_eq!(find_kid(&t3, "a").files, 2);
        assert!(ctx3.reused.load(Relaxed) >= 1);
    }

    #[test]
    fn index_keeps_top_files_and_survives_round_trip() {
        let t = tempdir().unwrap();
        let out = tempdir().unwrap();
        write(&t.path().join("d/x.bin"), 150_000);
        write(&t.path().join("d/y.bin"), 250_000);
        let idx = out.path().join("t.idx");
        scan_with(t.path(), None, Some(&idx));
        let prev = read_index(&idx).unwrap();
        let (_, top) = prev.get(fnv(&t.path().join("d"))).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(&*top[0].name, "x.bin".min("y.bin"));
        assert!(read_index(&out.path().join("missing.idx")).is_none());
        fs::write(out.path().join("bad.idx"), b"nope").unwrap();
        assert!(read_index(&out.path().join("bad.idx")).is_none());
    }

    #[test]
    fn tree_cache_round_trip_and_staleness() {
        let cache = tempdir().unwrap();
        let root = tempdir().unwrap();
        write(&root.path().join("d/x.bin"), 150_000);
        let idx = idx_path(cache.path(), root.path());
        let (tree, ctx) = scan_with(root.path(), None, Some(&idx));
        let tree = tree.unwrap();
        let meta = meta_for(root.path(), &ctx, &tree, 5);
        save_tree(cache.path(), root.path(), &meta, &tree).unwrap();
        let (m2, tree2) = load_cache(cache.path(), root.path()).unwrap();
        assert_eq!(m2.bytes, tree.size);
        assert_eq!(tree2.size, tree.size);
        assert!(load_prev_index(cache.path(), root.path(), now_secs()).is_some());
        // an index older than a day is refused
        assert!(load_prev_index(cache.path(), root.path(), now_secs() + MAX_CACHE_AGE_SECS + 60).is_none());
        assert_eq!(clear_cache(cache.path()), 2);
        assert!(load_cache(cache.path(), root.path()).is_none());
    }

    #[test]
    fn browsing_finds_nested_nodes_and_lists_rows() {
        let t = tempdir().unwrap();
        write(&t.path().join("a/b/c.bin"), 200_000);
        write(&t.path().join("a/top.bin"), 150_000);
        let tree = scan(t.path()).0.unwrap();
        let a = find(&tree, t.path(), &t.path().join("a")).unwrap();
        let r = rows(a, &t.path().join("a"));
        assert!(r.iter().any(|x| x.kind == "dir" && x.name == "b"));
        assert!(r.iter().any(|x| x.kind == "file" && x.name == "top.bin"));
        assert!(r.windows(2).all(|w| w[0].size >= w[1].size));
        assert!(find(&tree, t.path(), &t.path().join("nope")).is_none());
        assert!(find(&tree, t.path(), Path::new("/somewhere/else")).is_none());
    }
}
