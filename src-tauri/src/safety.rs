//! The only gate to deletion. Nothing is cleaned unless `Safety::check` returns Ok for it.
//!
//! Two layers, both required:
//!   allowlist - the path is matched by the rule that claims it, or was picked by the user
//!   blocklist - checked on the real path (symlinked ancestors resolved) and it always wins

use crate::rules::Rule;
use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum Denied {
    Malformed,
    Missing,
    Blocked(&'static str),
    NotAllowed,
    Symlinked,
}

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Denied::Malformed => write!(f, "not a valid absolute path"),
            Denied::Missing => write!(f, "does not exist any more"),
            Denied::Blocked(why) => write!(f, "protected: {why}"),
            Denied::NotAllowed => write!(f, "not covered by any cleanup rule"),
            Denied::Symlinked => write!(f, "a parent folder is a symlink"),
        }
    }
}

/// Compiled include/exclude globs of one rule.
pub struct RuleMatcher {
    include: Vec<glob::Pattern>,
    exclude: Vec<glob::Pattern>,
}

fn glob_opts() -> glob::MatchOptions {
    // Windows paths are case-insensitive: `AppData` and `appdata` are the same folder
    glob::MatchOptions { case_sensitive: !cfg!(windows), require_literal_separator: true, require_literal_leading_dot: false }
}

/// The real path with symlinks resolved. Windows' canonical form starts with `\\?\`, which nothing else
/// writes, so it is dropped to compare like any other path.
pub fn canon(p: &Path) -> std::io::Result<PathBuf> {
    let c = p.canonicalize()?;
    #[cfg(windows)]
    {
        let s = c.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(c)
}

/// A path in comparable form: as is on Unix; on Windows lower-cased with one kind of separator.
fn key(p: &Path) -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(p.to_string_lossy().replace('/', "\\").to_lowercase());
    #[cfg(not(windows))]
    p.to_path_buf()
}

/// `p` is `b` (same folder, ignoring case where the system does).
fn same(p: &Path, b: &Path) -> bool {
    key(p) == key(b)
}

/// `p` is `b` or inside it.
pub fn under(p: &Path, b: &Path) -> bool {
    key(p).starts_with(key(b))
}

pub fn expand_tilde(p: &str, home: &Path) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => format!("{}/{}", glob::Pattern::escape(&home.to_string_lossy()), rest),
        None => p.to_string(),
    }
}

impl RuleMatcher {
    pub fn new(rule: &Rule, home: &Path) -> RuleMatcher {
        let compile = |v: &[String]| v.iter().filter_map(|p| glob::Pattern::new(&expand_tilde(p, home)).ok()).collect();
        RuleMatcher { include: compile(&rule.paths), exclude: compile(&rule.exclude) }
    }

    /// From patterns that are already fully expanded (tokens and `~` replaced).
    pub fn from_expanded(include: &[String], exclude: &[String]) -> RuleMatcher {
        let compile = |v: &[String]| v.iter().filter_map(|p| glob::Pattern::new(p).ok()).collect();
        RuleMatcher { include: compile(include), exclude: compile(exclude) }
    }

    pub fn matches(&self, p: &Path) -> bool {
        let o = glob_opts();
        self.include.iter().any(|g| g.matches_path_with(p, o)) && !self.exclude.iter().any(|g| g.matches_path_with(p, o))
    }
}

/// Project artifact folders (node_modules, target, build, ...): the folder name must match, a marker file
/// must sit next to it, and it must be below a folder the user chose to scan.
pub struct ArtifactMatcher {
    dir_name: String,
    markers: Vec<String>,
    roots: Vec<PathBuf>,
}

impl ArtifactMatcher {
    pub fn new(rule: &Rule, roots: &[PathBuf]) -> ArtifactMatcher {
        ArtifactMatcher {
            dir_name: rule.dir_name.clone().unwrap_or_default(),
            markers: rule.markers.clone(),
            roots: roots.iter().map(|r| canon(r).unwrap_or_else(|_| r.clone())).collect(),
        }
    }

    pub fn matches(&self, real: &Path) -> bool {
        !self.dir_name.is_empty()
            && real.file_name().is_some_and(|n| *n == *self.dir_name)
            && std::fs::symlink_metadata(real).is_ok_and(|m| m.is_dir())
            && self.roots.iter().any(|r| under(real, r) && !same(real, r))
            && (self.markers.is_empty() || real.parent().is_some_and(|p| self.markers.iter().any(|m| p.join(m).exists())))
    }
}

pub enum Allowed<'a> {
    Rule(&'a RuleMatcher),
    Artifact(&'a ArtifactMatcher),
    UserPick,
}

pub struct Safety {
    home: PathBuf,
    /// Where apps are installed for all users (`/Applications`); a field so tests can use a temp folder.
    apps_dir: PathBuf,
}

/// Directories that are never cleaned *as a whole* (their children may be, if allowed).
#[cfg(not(any(windows, target_os = "linux")))]
fn exact_blocks(home: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut v: Vec<(PathBuf, &'static str)> = vec![
        ("/".into(), "the disk root"),
        ("/Users".into(), "the Users folder"),
        ("/Volumes".into(), "the Volumes folder"),
        ("/Applications".into(), "the Applications folder"),
        (home.to_path_buf(), "your home folder"),
    ];
    for (rel, why) in [
        ("Desktop", "your Desktop folder"),
        ("Documents", "your Documents folder"),
        ("Downloads", "your Downloads folder"),
        ("Pictures", "your Pictures folder"),
        ("Movies", "your Movies folder"),
        ("Music", "your Music folder"),
        ("Public", "your Public folder"),
        ("Library", "your Library folder"),
        ("Applications", "your Applications folder"),
        (".Trash", "the Trash folder itself"),
        ("Library/Application Support", "Application Support"),
        ("Library/Caches", "the Caches folder itself"),
        ("Library/Preferences", "Preferences"),
        ("Library/Containers", "app containers"),
        ("Library/Logs", "the Logs folder itself"),
    ] {
        v.push((home.join(rel), why));
    }
    v
}

/// System locations whose whole subtree is off limits (unless the user's home lives there).
#[cfg(not(any(windows, target_os = "linux")))]
fn system_blocks() -> Vec<(PathBuf, &'static str)> {
    [
        ("/System", "macOS system files"),
        ("/bin", "system binaries"),
        ("/sbin", "system binaries"),
        ("/etc", "system configuration"),
        ("/var", "system data"),
        ("/private", "system data"),
        ("/dev", "devices"),
        ("/cores", "system data"),
        ("/opt", "installed software"),
        ("/Library", "system-wide library"),
        ("/usr", "system software"),
    ]
    .iter()
    .map(|(p, w)| (PathBuf::from(p), *w))
    .collect()
}

/// Locations inside the home folder whose whole subtree is off limits, always.
#[cfg(not(any(windows, target_os = "linux")))]
fn home_blocks(home: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut v: Vec<(PathBuf, &'static str)> = Vec::new();
    for (rel, why) in [
        ("Library/Mobile Documents", "iCloud Drive"),
        ("Library/CloudStorage", "cloud-synced folders"),
        ("Library/Keychains", "keychains"),
        ("Library/Mail", "Mail"),
        ("Library/Messages", "Messages"),
        ("Library/Containers/com.apple.mail", "Mail"),
        ("Library/Containers/com.apple.MobileSMS", "Messages"),
        ("Library/Photos", "Photos"),
        ("Library/Safari", "Safari history and bookmarks"),
        ("Library/Calendars", "Calendars"),
        ("Library/Accounts", "account data"),
        ("Library/Cookies", "cookies"),
        ("Library/Group Containers", "shared app data"),
        ("Music/Music", "your Music library"),
        ("Music/iTunes", "your Music library"),
        ("Movies/TV", "your TV library"),
        (".ssh", "SSH keys"),
        (".gnupg", "GPG keys"),
        (".aws", "cloud credentials"),
        (".kube", "cluster credentials"),
    ] {
        v.push((home.join(rel), why));
    }
    v
}

/// Windows: folders that are never cleaned as a whole (their children may be, if allowed).
#[cfg(windows)]
fn exact_blocks(home: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut v: Vec<(PathBuf, &'static str)> = vec![(home.to_path_buf(), "your profile folder")];
    if let Some(users) = home.parent() {
        v.push((users.to_path_buf(), "the Users folder"));
    }
    for (rel, why) in [
        ("Desktop", "your Desktop folder"),
        ("Documents", "your Documents folder"),
        ("Downloads", "your Downloads folder"),
        ("Pictures", "your Pictures folder"),
        ("Videos", "your Videos folder"),
        ("Music", "your Music folder"),
        ("Favorites", "your Favorites"),
        ("Contacts", "your Contacts"),
        ("Links", "your Links"),
        ("Saved Games", "your saved games"),
        ("Searches", "your saved searches"),
        ("3D Objects", "your 3D Objects folder"),
        ("AppData", "app data"),
        ("AppData/Local", "app data"),
        ("AppData/Roaming", "app data"),
        ("AppData/LocalLow", "app data"),
        ("AppData/Local/Temp", "the Temp folder itself"),
        ("AppData/Local/Packages", "Store app data"),
        ("AppData/Local/Programs", "installed programs"),
        ("AppData/Local/Microsoft", "Windows app data"),
        ("AppData/Roaming/Microsoft", "Windows app data"),
    ] {
        v.push((home.join(rel), why));
    }
    v
}

/// Windows: system folders on any drive, off limits with everything in them.
#[cfg(windows)]
fn system_blocks() -> Vec<(PathBuf, &'static str)> {
    Vec::new() // matched by name on every drive in `windows_system_block`
}

#[cfg(windows)]
fn windows_system_block(p: &Path) -> Option<&'static str> {
    let first = p.components().find_map(|c| if let Component::Normal(n) = c { Some(n.to_string_lossy().to_lowercase()) } else { None })?;
    Some(match first.as_str() {
        "windows" => "Windows system files",
        "program files" | "program files (x86)" => "installed programs",
        "programdata" => "data for all users",
        "$recycle.bin" => "the Recycle Bin itself",
        "system volume information" => "system data",
        "recovery" | "boot" | "efi" => "system startup files",
        "pagefile.sys" | "hiberfil.sys" | "swapfile.sys" => "system memory files",
        _ => return None,
    })
}

/// Windows: places in the profile that are off limits with everything in them.
#[cfg(windows)]
fn home_blocks(home: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut v: Vec<(PathBuf, &'static str)> = Vec::new();
    for (rel, why) in [
        ("AppData/Roaming/Microsoft/Credentials", "saved Windows passwords"),
        ("AppData/Local/Microsoft/Credentials", "saved Windows passwords"),
        ("AppData/Roaming/Microsoft/Protect", "encryption keys"),
        ("AppData/Roaming/Microsoft/Crypto", "encryption keys"),
        ("AppData/Roaming/Microsoft/SystemCertificates", "certificates"),
        ("AppData/Local/Microsoft/Vault", "saved passwords"),
        ("AppData/Roaming/Microsoft/Vault", "saved passwords"),
        ("AppData/Local/Microsoft/Outlook", "Outlook mail"),
        ("AppData/Roaming/Microsoft/Windows/Start Menu", "the Start menu"),
        ("iCloudDrive", "iCloud Drive"),
        ("Dropbox", "a cloud-synced folder"),
        (".ssh", "SSH keys"),
        (".gnupg", "GPG keys"),
        (".aws", "cloud credentials"),
        (".azure", "cloud credentials"),
        (".kube", "cluster credentials"),
    ] {
        v.push((home.join(rel), why));
    }
    v
}

/// Windows: names directly in the profile that are always off limits (registry files, OneDrive folders).
#[cfg(windows)]
fn windows_profile_block(home: &Path, p: &Path) -> Option<&'static str> {
    let rel = key(p).strip_prefix(key(home)).ok()?.to_path_buf();
    let first = rel.components().next()?.as_os_str().to_string_lossy().into_owned();
    if first.starts_with("ntuser.") || first.starts_with("usrclass.") {
        return Some("your registry settings");
    }
    if first.starts_with("onedrive") {
        return Some("OneDrive");
    }
    None
}

/// Windows: the system drive (`C:`), where `Users` and `Windows` live.
#[cfg(windows)]
fn system_drive() -> String {
    std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_lowercase()
}

/// Component names that mark a protected subtree wherever they appear.
fn component_block(name: &str) -> Option<&'static str> {
    if name == ".git" {
        return Some("a git repository's history");
    }
    if name == "icloud drive" { // names arrive lower-cased
        return Some("iCloud Drive");
    }
    for (suffix, why) in [
        (".photoslibrary", "a Photos library"),
        (".tvlibrary", "a TV library"),
        (".musiclibrary", "a Music library"),
        (".keychain", "a keychain"),
        (".keychain-db", "a keychain"),
    ] {
        if name.ends_with(suffix) {
            return Some(why);
        }
    }
    None
}

/// A path below a mounted external drive (not the drive itself): picks there are allowed like in home.
#[cfg(not(any(windows, target_os = "linux")))]
fn on_other_volume(real: &Path) -> bool {
    real.strip_prefix("/Volumes").map(|r| r.components().count() >= 2).unwrap_or(false)
}

/// Linux: folders that are never cleaned as a whole (their children may be, if allowed).
#[cfg(target_os = "linux")]
fn exact_blocks(home: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut v: Vec<(PathBuf, &'static str)> = vec![
        ("/".into(), "the disk root"),
        ("/home".into(), "the home folders"),
        ("/media".into(), "the drives folder"),
        ("/mnt".into(), "the mounts folder"),
        ("/run/media".into(), "the drives folder"),
        (home.to_path_buf(), "your home folder"),
    ];
    for (rel, why) in [
        ("Desktop", "your Desktop folder"),
        ("Documents", "your Documents folder"),
        ("Downloads", "your Downloads folder"),
        ("Pictures", "your Pictures folder"),
        ("Videos", "your Videos folder"),
        ("Music", "your Music folder"),
        ("Public", "your Public folder"),
        ("Templates", "your Templates folder"),
        (".cache", "the cache folder itself"),
        (".config", "your settings"),
        (".local", "app data"),
        (".local/share", "app data"),
        (".local/bin", "your programs"),
        (".local/share/Trash", "the Trash folder itself"),
        (".local/share/Trash/files", "the Trash folder itself"),
        ("snap", "Snap app data"),
    ] {
        v.push((home.join(rel), why));
    }
    v
}

/// Linux: system locations whose whole subtree is off limits.
#[cfg(target_os = "linux")]
fn system_blocks() -> Vec<(PathBuf, &'static str)> {
    [
        ("/bin", "system programs"),
        ("/sbin", "system programs"),
        ("/lib", "system libraries"),
        ("/lib32", "system libraries"),
        ("/lib64", "system libraries"),
        ("/usr", "system software"),
        ("/etc", "system configuration"),
        ("/var", "system data"),
        ("/boot", "system startup files"),
        ("/efi", "system startup files"),
        ("/dev", "devices"),
        ("/proc", "system state"),
        ("/sys", "system state"),
        ("/run", "system state"),
        ("/opt", "installed software"),
        ("/srv", "served data"),
        ("/root", "the administrator's home"),
        ("/snap", "installed Snap apps"),
    ]
    .iter()
    .map(|(p, w)| (PathBuf::from(p), *w))
    .collect()
}

/// Linux: places in the home folder that are off limits with everything in them.
#[cfg(target_os = "linux")]
fn home_blocks(home: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut v: Vec<(PathBuf, &'static str)> = Vec::new();
    for (rel, why) in [
        (".ssh", "SSH keys"),
        (".gnupg", "GPG keys"),
        (".pki", "certificates"),
        (".password-store", "saved passwords"),
        (".local/share/keyrings", "saved passwords"),
        (".local/share/kwalletd", "saved passwords"),
        (".mozilla", "Firefox profiles"),
        (".thunderbird", "Thunderbird mail"),
        (".config/google-chrome", "Chrome profiles"),
        (".config/chromium", "Chromium profiles"),
        (".config/BraveSoftware", "Brave profiles"),
        (".config/microsoft-edge", "Edge profiles"),
        (".local/share/evolution", "Evolution mail"),
        (".aws", "cloud credentials"),
        (".azure", "cloud credentials"),
        (".kube", "cluster credentials"),
        (".docker", "Docker credentials"),
        ("Dropbox", "a cloud-synced folder"),
        ("Nextcloud", "a cloud-synced folder"),
        ("OneDrive", "a cloud-synced folder"),
    ] {
        v.push((home.join(rel), why));
    }
    v
}

/// Linux: a path below a drive mounted in `/media/<user>/<drive>`, `/run/media/<user>/<drive>` or
/// `/mnt/<drive>` (not the mount point itself).
#[cfg(target_os = "linux")]
fn on_other_volume(real: &Path) -> bool {
    let depth = |prefix: &str| real.strip_prefix(prefix).ok().map(|r| r.components().count());
    depth("/media").is_some_and(|n| n >= 3) || depth("/run/media").is_some_and(|n| n >= 3) || depth("/mnt").is_some_and(|n| n >= 2)
}

/// Windows: a path on another drive than the system drive, below its root.
#[cfg(windows)]
fn on_other_volume(real: &Path) -> bool {
    use std::path::Prefix;
    let Some(Component::Prefix(pre)) = real.components().next() else { return false };
    let drive = match pre.kind() {
        Prefix::Disk(d) | Prefix::VerbatimDisk(d) => format!("{}:", d as char).to_lowercase(),
        _ => return false, // network shares are not cleaned
    };
    drive != system_drive() && real.components().filter(|c| matches!(c, Component::Normal(_))).count() >= 1
}

impl Safety {
    pub fn new(home: PathBuf) -> Safety {
        // Compare against the canonical home so /var/... vs /private/var/... (and tests in temp dirs) agree.
        Safety { home: canon(&home).unwrap_or(home), apps_dir: PathBuf::from("/Applications") }
    }

    /// Use another folder as the shared Applications folder (tests).
    pub fn with_apps_dir(mut self, dir: PathBuf) -> Safety {
        self.apps_dir = canon(&dir).unwrap_or(dir);
        self
    }

    /// An app bundle installed for all users: `<Applications>/X.app` or one vendor folder down
    /// (`<Applications>/Vendor/X.app`). Never the folder itself and never anything inside a bundle.
    fn is_installed_app(&self, real: &Path) -> bool {
        let Ok(rel) = real.strip_prefix(&self.apps_dir) else { return false };
        let parts: Vec<_> = rel.components().collect();
        let in_place = parts.len() == 1 || (parts.len() == 2 && !Path::new(parts[0].as_os_str()).extension().is_some_and(|x| x == "app"));
        in_place && real.extension().is_some_and(|x| x == "app") && real.join("Contents").is_dir()
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    fn blocked(&self, p: &Path, allow_system_rules: bool) -> Option<&'static str> {
        for (b, why) in exact_blocks(&self.home) {
            if same(p, &b) {
                return Some(why);
            }
        }
        #[cfg(windows)]
        {
            if p.parent().is_none() || p.components().filter(|c| matches!(c, Component::Normal(_))).count() == 0 {
                return Some("a drive's root");
            }
            if !under(p, &self.home) {
                if let Some(why) = windows_system_block(p) {
                    // the Recycle Bin rule may empty the bin; nothing in it can be picked by hand
                    let recycle = p.components().any(|c| c.as_os_str().eq_ignore_ascii_case("$recycle.bin"));
                    if !(allow_system_rules && recycle) {
                        return Some(why);
                    }
                }
            }
            if let Some(why) = windows_profile_block(&self.home, p) {
                return Some(why);
            }
        }
        if !under(p, &self.home) {
            for (b, why) in system_blocks() {
                // Linux mounts drives below /run/media, inside the protected /run
                if cfg!(target_os = "linux") && on_other_volume(p) {
                    continue;
                }
                if under(p, &b) {
                    // `/usr/local` and `/Library/Caches` may be named by a rule, never picked by hand.
                    if allow_system_rules && (p.starts_with("/usr/local") || p.starts_with("/Library/Caches") || p.starts_with("/Library/Logs")) {
                        continue;
                    }
                    return Some(why);
                }
            }
        }
        for (b, why) in home_blocks(&self.home) {
            if under(p, &b) {
                return Some(why);
            }
        }
        // /Volumes/<name> itself (a mount point) is never a target.
        if let Ok(rest) = p.strip_prefix("/Volumes") {
            if rest.components().count() == 1 {
                return Some("a mounted volume");
            }
        }
        // Linux: the mount points of drives (`/media/<user>/<drive>`, `/mnt/<drive>`) and the folders above them
        #[cfg(target_os = "linux")]
        for (prefix, depth) in [("/media", 2), ("/run/media", 2), ("/mnt", 1)] {
            if p.strip_prefix(prefix).is_ok_and(|r| r.components().count() <= depth) {
                return Some("a mounted drive");
            }
        }
        for c in p.components() {
            if let Component::Normal(n) = c {
                if let Some(why) = component_block(&n.to_string_lossy().to_lowercase()) {
                    return Some(why);
                }
            }
        }
        None
    }

    /// Resolve symlinked ancestors. The final component is *not* followed: moving a symlink to the
    /// Trash moves the link, never its target.
    fn resolve(&self, p: &Path) -> Result<PathBuf, Denied> {
        if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir | Component::CurDir)) {
            return Err(Denied::Malformed);
        }
        let name = p.file_name().ok_or(Denied::Malformed)?;
        let parent = p.parent().ok_or(Denied::Malformed)?;
        let real_parent = canon(parent).map_err(|_| Denied::Missing)?;
        let real = real_parent.join(name);
        std::fs::symlink_metadata(&real).map_err(|_| Denied::Missing)?;
        Ok(real)
    }

    /// Why a folder can't be used as a tool location (`None` when it can). A location is the *root* that
    /// rule globs are applied under, so it must be a real folder below home or on an external volume, and
    /// never home itself, a protected folder or a system path.
    pub fn location_problem(&self, p: &Path) -> Option<String> {
        if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Some("not an absolute path".into());
        }
        let Ok(real) = canon(p) else { return Some("does not exist".into()) };
        if !real.is_dir() {
            return Some("not a folder".into());
        }
        for q in [p, real.as_path()] {
            if let Some(why) = self.blocked(q, false) {
                if !self.appdata_root(q) {
                    return Some(format!("protected: {why}"));
                }
            }
        }
        if !(under(&real, &self.home) || on_other_volume(&real)) {
            return Some("outside your home folder".into());
        }
        None
    }

    /// Windows keeps app data in `AppData\\Local`, `AppData\\Roaming` and `Temp`. They are never cleaned as
    /// a whole, but rules may name things inside them, so they are fine as tool locations.
    #[cfg(windows)]
    fn appdata_root(&self, p: &Path) -> bool {
        ["AppData/Local", "AppData/Roaming", "AppData/LocalLow", "AppData/Local/Temp"].iter().any(|r| same(p, &self.home.join(r)))
    }
    /// Linux keeps caches and app data in the XDG folders. Like AppData on Windows, they are never cleaned as a
    /// whole but are fine as tool locations.
    #[cfg(target_os = "linux")]
    fn appdata_root(&self, p: &Path) -> bool {
        [".cache", ".config", ".local/share"].iter().any(|r| same(p, &self.home.join(r)))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    fn appdata_root(&self, _p: &Path) -> bool {
        false
    }

    /// Home itself or one of the folders that are never cleaned as a whole (Desktop, Documents, Library, ...).
    pub fn is_protected_root(&self, p: &Path) -> bool {
        same(p, &self.home) || exact_blocks(&self.home).iter().any(|(b, _)| same(b, p))
    }

    /// Why `p` is protected, if it is.
    pub fn protected_reason(&self, p: &Path) -> Option<&'static str> {
        self.blocked(p, false)
    }

    /// Returns the real path that may be acted on.
    pub fn check(&self, path: &Path, allowed: &Allowed) -> Result<PathBuf, Denied> {
        let rule_mode = matches!(allowed, Allowed::Rule(_)); // only glob rules may name /usr/local etc.
        // The blocklist always wins, on the spelling we were given and on the resolved path. The lexical
        // check comes first so `/` and friends are reported as protected rather than malformed.
        if path.is_absolute() {
            if let Some(why) = self.blocked(path, rule_mode) {
                return Err(Denied::Blocked(why));
            }
        }
        let real = self.resolve(path)?;
        for p in [path, real.as_path()] {
            if let Some(why) = self.blocked(p, rule_mode) {
                return Err(Denied::Blocked(why));
            }
        }
        match allowed {
            Allowed::Rule(m) => {
                if !m.matches(&real) {
                    return Err(if m.matches(path) { Denied::Symlinked } else { Denied::NotAllowed });
                }
            }
            Allowed::Artifact(m) => {
                if !m.matches(&real) {
                    return Err(if m.matches(path) { Denied::Symlinked } else { Denied::NotAllowed });
                }
            }
            Allowed::UserPick => {
                if !(under(&real, &self.home) || on_other_volume(&real) || self.is_installed_app(&real)) {
                    return Err(Denied::NotAllowed);
                }
            }
        }
        Ok(real)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Registry;
    use std::fs;
    use tempfile::TempDir;

    /// Windows: the profile's protected places hold regardless of how a path is spelled.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_blocklist_covers_system_folders_keys_and_mount_points() {
        let t = TempDir::new().unwrap();
        let home = canon(t.path()).unwrap().join("home");
        for d in [".cache/app", ".config/app", ".ssh", "Documents", ".local/share/keyrings"] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        let s = Safety::new(home.clone());
        for p in ["/", "/etc/hosts", "/usr/lib/x", "/var/log/syslog", "/boot/vmlinuz", "/proc/1", "/home", "/media/me/USB", "/run/media/me/USB", "/mnt/data"] {
            assert!(s.protected_reason(Path::new(p)).is_some(), "{p} must be protected");
        }
        for rel in ["", ".cache", ".config", ".local/share", "Documents", ".ssh/id_ed25519", ".local/share/keyrings/login.keyring", ".config/google-chrome/Default/Cookies"] {
            assert!(s.protected_reason(&home.join(rel)).is_some(), "~/{rel} must be protected");
        }
        for rel in [".cache/app", ".config/app/settings.json", "Documents/report.pdf"] {
            assert!(s.protected_reason(&home.join(rel)).is_none(), "~/{rel} may be picked");
        }
        assert!(s.protected_reason(Path::new("/media/me/USB/old.iso")).is_none(), "files on a drive may be picked");
        assert!(s.protected_reason(Path::new("/run/media/me/USB/old.iso")).is_none(), "also where some desktops mount drives");
        assert!(s.location_problem(&home.join(".cache")).is_none(), "the cache folder works as a tool location");
    }

    #[cfg(windows)]
    #[test]
    fn windows_blocklist_ignores_case_and_covers_system_folders() {
        let t = TempDir::new().unwrap();
        let home = canon(t.path()).unwrap().join("profile");
        for d in [
            "Desktop", "Documents", "Downloads/sub", "AppData/Local/Temp/old", "AppData/Roaming/Microsoft/Credentials", ".ssh",
            "OneDrive - Personal/docs", "Projects/app/.git", "AppData/Local/Google/Chrome/User Data/Default/Cache",
        ] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        for f in ["NTUSER.DAT", "Downloads/sub/old.zip", ".ssh/id_ed25519", "AppData/Roaming/Microsoft/Credentials/blob", "OneDrive - Personal/docs/a.txt", "AppData/Local/Temp/old/a.tmp"] {
            fs::write(home.join(f), b"x").unwrap();
        }
        let s = Safety::new(home.clone());
        let pick = |rel: &str| s.check(&home.join(rel), &Allowed::UserPick);
        assert!(pick("Downloads/sub/old.zip").is_ok(), "an ordinary pick in the profile is fine");
        assert!(pick("AppData/Local/Temp/old/a.tmp").is_ok());
        for rel in ["Desktop", "DESKTOP", "Documents", "AppData", "appdata/local", "AppData/Local/Temp"] {
            assert!(matches!(pick(rel), Err(Denied::Blocked(_))), "{rel} is never removed as a whole");
        }
        for rel in ["NTUSER.DAT", "ntuser.dat", ".ssh/id_ed25519", "AppData/Roaming/Microsoft/Credentials/blob", "APPDATA/ROAMING/MICROSOFT/CREDENTIALS/blob", "OneDrive - Personal/docs/a.txt", "Projects/app/.git"] {
            assert!(matches!(pick(rel), Err(Denied::Blocked(_))), "{rel} is protected");
        }
        assert!(s.protected_reason(&home).is_some(), "the profile itself");
        assert!(s.protected_reason(&home.parent().unwrap().to_path_buf()).is_some(), "the folder holding profiles");
        for p in [r"C:\", r"C:\Windows\System32\drivers", r"C:\Program Files\App", r"D:\Program Files (x86)\App", r"C:\ProgramData\x", r"c:\windows\temp\a"] {
            assert!(s.protected_reason(Path::new(p)).is_some(), "{p}");
        }
        // a cache folder a rule names is allowed, spelled any way
        let rule: Rule = toml::from_str(&format!("id=\"t.c\"\nname=\"C\"\ncategory=\"Browsers\"\npaths=[\"~/AppData/Local/Google/Chrome/User Data/*/Cache\"]\nrisk=\"safe\"\ndescription=\"x\"\nregenerates=\"x\"\nmethod=\"trash\"\n")).unwrap();
        let m = RuleMatcher::new(&rule, &home);
        assert!(s.check(&home.join("AppData/Local/Google/Chrome/User Data/Default/Cache"), &Allowed::Rule(&m)).is_ok());
        assert!(s.check(&home.join("appdata/local/google/chrome/user data/Default/Cache"), &Allowed::Rule(&m)).is_ok(), "case does not matter on Windows");
    }

    #[cfg(windows)]
    #[test]
    fn windows_app_data_folders_work_as_locations_but_documents_does_not() {
        let t = TempDir::new().unwrap();
        let home = canon(t.path()).unwrap().join("profile");
        for d in ["AppData/Local", "AppData/Roaming", "AppData/Local/Temp", "Documents"] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        let s = Safety::new(home.clone());
        for d in ["AppData/Local", "AppData/Roaming", "AppData/Local/Temp"] {
            assert_eq!(s.location_problem(&home.join(d)), None, "{d}");
        }
        assert!(s.location_problem(&home.join("Documents")).is_some());
        assert!(s.location_problem(&home).is_some());
    }

    #[cfg(windows)]
    #[test]
    fn windows_canonical_paths_lose_the_verbatim_prefix() {
        let t = TempDir::new().unwrap();
        let c = canon(t.path()).unwrap();
        assert!(!c.to_string_lossy().starts_with(r"\\?\"), "{}", c.display());
        assert!(c.is_absolute());
    }

    #[cfg_attr(not(target_os = "macos"), ignore = "macOS paths and rules; other systems are tested separately")]
    #[test]
    fn a_user_may_pick_an_installed_app_bundle_but_nothing_else_in_applications() {
        let t = TempDir::new().unwrap();
        let root = crate::safety::canon(t.path()).unwrap();
        let (home, apps) = (root.join("home"), root.join("Applications"));
        for d in ["Example.app/Contents/MacOS", "Vendor/Tool.app/Contents", "Vendor/Deep/Nested.app/Contents", "Folder/data"] {
            fs::create_dir_all(apps.join(d)).unwrap();
        }
        fs::create_dir_all(&home).unwrap();
        fs::write(apps.join("notes.txt"), b"x").unwrap();
        let s = Safety::new(home).with_apps_dir(apps.clone());
        // temp folders live under /private, which the blocklist rightly refuses, so test the rule itself;
        // for the real /Applications the full check also runs it (see UserPick in `check`)
        let ok = |p: &str| s.is_installed_app(&apps.join(p));
        assert!(ok("Example.app") && ok("Vendor/Tool.app"));
        assert!(!ok("Example.app/Contents/MacOS"), "nothing inside a bundle");
        assert!(!ok("Vendor/Deep/Nested.app"), "only one vendor folder down");
        assert!(!ok("Folder") && !ok("Folder/data") && !ok("notes.txt"), "only app bundles");
        assert!(!ok("Vendor"), "not a vendor folder itself");
        assert!(Safety::new(PathBuf::from("/Users/x")).check(Path::new("/Applications"), &Allowed::UserPick).is_err(), "never the folder");
    }

    /// A throw-away "home" with the folders the blocklist knows about.
    fn fixture() -> (TempDir, Safety) {
        let t = tempfile::tempdir().unwrap();
        let home = crate::safety::canon(t.path()).unwrap().join("home");
        for d in [
            "Desktop", "Documents", "Pictures/Photos Library.photoslibrary/database", "Music/Music/Media", "Library/Caches/com.example.app",
            "Library/Caches/com.apple.thing", "Library/Mobile Documents/com~apple~CloudDocs", "Library/Keychains", "Library/Logs", ".ssh",
            "Projects/app/.git/objects", "Projects/app/build", ".Trash", "Library/Caches/Google/Chrome/Default",
        ] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        fs::write(home.join("Documents/report.pdf"), b"x").unwrap();
        fs::write(home.join(".ssh/id_ed25519"), b"x").unwrap();
        fs::write(home.join("Library/Caches/com.example.app/blob"), b"x").unwrap();
        fs::write(home.join("Library/Keychains/login.keychain-db"), b"x").unwrap();
        let s = Safety::new(home);
        (t, s)
    }

    fn rule(toml_paths: &str, exclude: &str) -> Rule {
        let src = format!(
            "schema_version = 1\nrules_version = \"t\"\n[[rule]]\nid=\"t.r\"\nname=\"T\"\ncategory=\"App caches\"\npaths={toml_paths}\nexclude={exclude}\nrisk=\"safe\"\ndescription=\"Deletes test caches only.\"\nregenerates=\"yes\"\nmethod=\"trash\"\n"
        );
        Registry::parse(&src).unwrap().rules.remove(0)
    }

    #[cfg_attr(not(target_os = "macos"), ignore = "macOS paths and rules; other systems are tested separately")]
    #[test]
    fn hard_blocklist_wins_for_user_picks() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        let pick = Allowed::UserPick;
        for (p, why) in [
            (Path::new("/").to_path_buf(), "root"),
            (Path::new("/System/Library").to_path_buf(), "system"),
            (Path::new("/usr/bin").to_path_buf(), "usr"),
            (Path::new("/Library/Caches").to_path_buf(), "/Library"),
            (h.clone(), "home"),
            (h.join("Desktop"), "Desktop root"),
            (h.join("Documents"), "Documents root"),
            (h.join("Pictures"), "Pictures root"),
            (h.join("Pictures/Photos Library.photoslibrary"), "photos library"),
            (h.join("Pictures/Photos Library.photoslibrary/database"), "inside photos library"),
            (h.join("Music/Music/Media"), "music library"),
            (h.join("Library/Mobile Documents/com~apple~CloudDocs"), "iCloud"),
            (h.join("Library/Keychains/login.keychain-db"), "keychain"),
            (h.join(".ssh/id_ed25519"), "ssh key"),
            (h.join("Projects/app/.git"), ".git"),
            (h.join("Projects/app/.git/objects"), "inside .git"),
            (h.join("Library/Caches"), "caches root"),
        ] {
            let r = s.check(&p, &pick);
            assert!(matches!(r, Err(Denied::Blocked(_))) || matches!(r, Err(Denied::Missing)) && p.starts_with("/Library"), "{why}: {p:?} -> {r:?}");
        }
    }

    #[test]
    fn user_picks_inside_normal_folders_are_allowed_but_only_under_home_or_volumes() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        assert!(s.check(&h.join("Documents/report.pdf"), &Allowed::UserPick).is_ok());
        assert!(s.check(&h.join("Projects/app/build"), &Allowed::UserPick).is_ok());
        // outside home: a real, existing directory that is not blocked by name still must be refused
        let other = tempfile::tempdir().unwrap();
        assert!(s.check(other.path(), &Allowed::UserPick).is_err(), "paths outside home/volumes are refused");
    }

    #[cfg_attr(not(target_os = "macos"), ignore = "macOS paths and rules; other systems are tested separately")]
    #[test]
    fn rule_allowlist_requires_a_glob_match_and_respects_exclude() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        let r = rule("[\"~/Library/Caches/*\"]", "[\"~/Library/Caches/com.apple.*\"]");
        let m = RuleMatcher::new(&r, s.home());
        let a = Allowed::Rule(&m);
        assert!(s.check(&h.join("Library/Caches/com.example.app"), &a).is_ok());
        assert_eq!(s.check(&h.join("Library/Caches/com.apple.thing"), &a), Err(Denied::NotAllowed), "excluded");
        assert_eq!(s.check(&h.join("Documents/report.pdf"), &a), Err(Denied::NotAllowed), "not in rule");
        // `*` never crosses a path separator
        assert_eq!(s.check(&h.join("Library/Caches/com.example.app/blob"), &a), Err(Denied::NotAllowed));
    }

    #[cfg_attr(not(target_os = "macos"), ignore = "macOS paths and rules; other systems are tested separately")]
    #[test]
    fn blocklist_beats_a_rule_that_matches() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        // A (wrong) rule that claims Documents and .git must still be refused.
        let r = rule("[\"~/Documents\", \"~/Projects/app/.git\", \"~/.ssh/*\", \"~/Library/Keychains/*\"]", "[]");
        let m = RuleMatcher::new(&r, s.home());
        let a = Allowed::Rule(&m);
        for p in ["Documents", "Projects/app/.git", ".ssh/id_ed25519", "Library/Keychains/login.keychain-db"] {
            assert!(matches!(s.check(&h.join(p), &a), Err(Denied::Blocked(_))), "{p}");
        }
    }

    #[test]
    fn malformed_and_missing_paths_are_refused() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        assert_eq!(s.check(Path::new("relative/path"), &Allowed::UserPick), Err(Denied::Malformed));
        assert_eq!(s.check(&h.join("Documents/../.ssh/id_ed25519"), &Allowed::UserPick), Err(Denied::Malformed));
        assert_eq!(s.check(&h.join("Documents/nope.txt"), &Allowed::UserPick), Err(Denied::Missing));
    }

    #[cfg_attr(not(target_os = "macos"), ignore = "macOS paths and rules; other systems are tested separately")]
    #[test]
    fn symlinked_ancestors_cannot_smuggle_a_blocked_target() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        // ~/Projects/link -> ~/.ssh : picking ~/Projects/link/id_ed25519 must resolve to .ssh and be blocked.
        crate::testutil::symlink(&h.join(".ssh"), &h.join("Projects/link")).unwrap();
        assert!(matches!(s.check(&h.join("Projects/link/id_ed25519"), &Allowed::UserPick), Err(Denied::Blocked(_))));
        // A rule path that only matches lexically (Caches/evil -> Documents) is refused for rules.
        crate::testutil::symlink(&h.join("Documents"), &h.join("Library/Caches/evil")).unwrap();
        let r = rule("[\"~/Library/Caches/*/*\"]", "[]");
        let m = RuleMatcher::new(&r, s.home());
        assert!(s.check(&h.join("Library/Caches/evil/report.pdf"), &Allowed::Rule(&m)).is_err());
        // A symlink as the *final* component is fine: only the link itself would be trashed.
        let r2 = rule("[\"~/Library/Caches/*\"]", "[]");
        let m2 = RuleMatcher::new(&r2, s.home());
        assert!(s.check(&h.join("Library/Caches/evil"), &Allowed::Rule(&m2)).is_ok());
    }

    #[test]
    fn every_shipped_rule_can_act_on_its_own_paths_and_none_is_blocked() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        let reg = Registry::macos().unwrap();
        let mut checked = 0;
        let locs = crate::locate::defaults(&reg.locations, s.home());
        for r in reg.rules.iter().filter(|r| matches!(r.kind, crate::rules::Kind::Path | crate::rules::Kind::Dynamic | crate::rules::Kind::Tool)) {
            let ex = |v: &[String]| v.iter().filter_map(|p| crate::locate::expand(p, &locs, s.home())).collect::<Vec<_>>();
            let m = RuleMatcher::from_expanded(&ex(&r.paths), &ex(&r.exclude));
            for p in &r.paths {
                // default locations, with every wildcard replaced by a concrete name
                let mut raw = p.clone();
                for t in crate::locate::tokens(p) {
                    let d = locs[t].path.as_ref().unwrap();
                    raw = raw.replace(&format!("{{{t}}}"), &d.to_string_lossy());
                }
                let concrete = PathBuf::from(raw.strip_prefix("~/").map(|r| h.join(r).to_string_lossy().into_owned()).unwrap_or(raw).replace('*', "x"));
                fs::create_dir_all(&concrete).unwrap();
                let res = s.check(&concrete, &Allowed::Rule(&m));
                assert!(res.is_ok(), "rule {} path {p}: {res:?}", r.id);
                checked += 1;
            }
        }
        assert!(checked > 40, "{checked}");
    }

    #[cfg_attr(not(target_os = "macos"), ignore = "macOS paths and rules; other systems are tested separately")]
    #[test]
    fn browser_rules_never_reach_profiles_cookies_or_history() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        let reg = Registry::macos().unwrap();
        let base = h.join("Library/Application Support");
        for (rule, dir) in [
            ("browser.chrome", "Google/Chrome"),
            ("browser.brave", "BraveSoftware/Brave-Browser"),
            ("browser.arc", "Arc/User Data"),
            ("browser.edge", "Microsoft Edge"),
            ("browser.vivaldi", "Vivaldi"),
            ("browser.chromium", "Chromium"),
        ] {
            let m = RuleMatcher::new(reg.get(rule).unwrap(), s.home());
            for secret in ["Default/Cookies", "Default/History", "Default/Login Data", "Default/Bookmarks", "Default", "Local State", "Default/Extensions"] {
                let p = base.join(dir).join(secret);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(&p, b"x").unwrap_or_else(|_| fs::create_dir_all(&p).unwrap());
                assert!(s.check(&p, &Allowed::Rule(&m)).is_err(), "{rule} must not match {secret}");
            }
            let cache = base.join(dir).join("Default/Cache");
            fs::create_dir_all(&cache).unwrap();
            assert!(s.check(&cache, &Allowed::Rule(&m)).is_ok(), "{rule} cache folder");
        }
        // Opera keeps its profile directly in its support folder.
        let m = RuleMatcher::new(reg.get("browser.opera").unwrap(), s.home());
        let opera = base.join("com.operasoftware.Opera");
        for secret in ["Cookies", "History", "Login Data", "Bookmarks"] {
            let p = opera.join(secret);
            fs::create_dir_all(opera.as_path()).unwrap();
            fs::write(&p, b"x").unwrap();
            assert!(s.check(&p, &Allowed::Rule(&m)).is_err(), "opera must not match {secret}");
        }
        fs::create_dir_all(opera.join("Cache")).unwrap();
        assert!(s.check(&opera.join("Cache"), &Allowed::Rule(&m)).is_ok());
        // VS Code: caches yes, settings and extensions no
        let m = RuleMatcher::new(reg.get("vscode.caches").unwrap(), s.home());
        for keep in ["User/settings.json", "User/workspaceStorage", "extensions"] {
            let p = base.join("Code").join(keep);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::create_dir_all(&p).unwrap_or(());
            assert!(s.check(&p, &Allowed::Rule(&m)).is_err(), "vscode must not match {keep}");
        }
        // Firefox profile data (places.sqlite, cookies.sqlite) lives in Application Support, not Caches.
        let m = RuleMatcher::new(reg.get("browser.firefox").unwrap(), s.home());
        let prof = base.join("Firefox/Profiles/abc.default/places.sqlite");
        fs::create_dir_all(prof.parent().unwrap()).unwrap();
        fs::write(&prof, b"x").unwrap();
        assert!(s.check(&prof, &Allowed::Rule(&m)).is_err());
    }

    #[cfg_attr(not(target_os = "macos"), ignore = "macOS paths and rules; other systems are tested separately")]
    #[test]
    fn generic_app_cache_rule_skips_apple_and_browser_caches() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        let reg = Registry::macos().unwrap();
        let m = RuleMatcher::new(reg.get("appcache.library_caches").unwrap(), s.home());
        assert!(s.check(&h.join("Library/Caches/com.example.app"), &Allowed::Rule(&m)).is_ok());
        for skip in ["com.apple.thing", "Google"] {
            let p = h.join("Library/Caches").join(skip);
            fs::create_dir_all(&p).unwrap();
            assert!(s.check(&p, &Allowed::Rule(&m)).is_err(), "{skip}");
        }
    }

    #[test]
    fn artifact_matcher_requires_name_marker_and_chosen_root() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        let reg = Registry::macos().unwrap();
        let rule = reg.get("artifact.node_modules").unwrap();
        fs::create_dir_all(h.join("Projects/app/node_modules/x")).unwrap();
        fs::write(h.join("Projects/app/package.json"), b"{}").unwrap();
        fs::create_dir_all(h.join("Projects/lonely/node_modules")).unwrap();
        let roots = vec![h.join("Projects")];
        let m = ArtifactMatcher::new(rule, &roots);
        let a = Allowed::Artifact(&m);
        assert!(s.check(&h.join("Projects/app/node_modules"), &a).is_ok());
        assert!(s.check(&h.join("Projects/lonely/node_modules"), &a).is_err(), "no package.json");
        assert!(s.check(&h.join("Projects/app/node_modules/x"), &a).is_err(), "wrong folder name");
        assert!(s.check(&h.join("Projects/app/package.json"), &a).is_err());
        let other = ArtifactMatcher::new(rule, &[h.join("Documents")]);
        assert!(s.check(&h.join("Projects/app/node_modules"), &Allowed::Artifact(&other)).is_err(), "outside the chosen roots");
        let root_itself = ArtifactMatcher::new(rule, &[h.join("Projects/app/node_modules")]);
        assert!(s.check(&h.join("Projects/app/node_modules"), &Allowed::Artifact(&root_itself)).is_err());
    }
}
