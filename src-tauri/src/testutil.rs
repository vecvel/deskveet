//! Helpers for tests that need the same filesystem tricks on every platform (always on temporary folders).

use std::fs::File;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

/// A symbolic link at `link` pointing to `target`. Windows needs to know whether the target is a folder.
pub fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return if target.is_dir() { std::os::windows::fs::symlink_dir(target, link) } else { std::os::windows::fs::symlink_file(target, link) };
}

/// Set the modification time of a file or folder to `secs` (unix seconds).
pub fn set_mtime(p: &Path, secs: u64) {
    let t = UNIX_EPOCH + Duration::from_secs(secs);
    #[cfg(unix)]
    let f = File::open(p);
    #[cfg(windows)]
    let f = {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, and FILE_FLAG_BACKUP_SEMANTICS so folders can be opened too
        File::options().access_mode(0x100).custom_flags(0x0200_0000).open(p)
    };
    f.and_then(|f| f.set_modified(t)).unwrap_or_else(|e| panic!("set_mtime {}: {e}", p.display()));
}
