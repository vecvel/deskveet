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
    glob::MatchOptions { case_sensitive: true, require_literal_separator: true, require_literal_leading_dot: false }
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
            roots: roots.iter().map(|r| r.canonicalize().unwrap_or_else(|_| r.clone())).collect(),
        }
    }

    pub fn matches(&self, real: &Path) -> bool {
        !self.dir_name.is_empty()
            && real.file_name().is_some_and(|n| *n == *self.dir_name)
            && std::fs::symlink_metadata(real).is_ok_and(|m| m.is_dir())
            && self.roots.iter().any(|r| real.starts_with(r) && real != r)
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
}

/// Directories that are never cleaned *as a whole* (their children may be, if allowed).
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

/// Component names that mark a protected subtree wherever they appear.
fn component_block(name: &str) -> Option<&'static str> {
    if name == ".git" {
        return Some("a git repository's history");
    }
    if name == "iCloud Drive" {
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

impl Safety {
    pub fn new(home: PathBuf) -> Safety {
        // Compare against the canonical home so /var/... vs /private/var/... (and tests in temp dirs) agree.
        Safety { home: home.canonicalize().unwrap_or(home) }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    fn blocked(&self, p: &Path, allow_system_rules: bool) -> Option<&'static str> {
        for (b, why) in exact_blocks(&self.home) {
            if p == b {
                return Some(why);
            }
        }
        if !p.starts_with(&self.home) {
            for (b, why) in system_blocks() {
                if p.starts_with(&b) {
                    // `/usr/local` and `/Library/Caches` may be named by a rule, never picked by hand.
                    if allow_system_rules && (p.starts_with("/usr/local") || p.starts_with("/Library/Caches") || p.starts_with("/Library/Logs")) {
                        continue;
                    }
                    return Some(why);
                }
            }
        }
        for (b, why) in home_blocks(&self.home) {
            if p.starts_with(&b) {
                return Some(why);
            }
        }
        // /Volumes/<name> itself (a mount point) is never a target.
        if let Ok(rest) = p.strip_prefix("/Volumes") {
            if rest.components().count() == 1 {
                return Some("a mounted volume");
            }
        }
        for c in p.components() {
            if let Component::Normal(n) = c {
                if let Some(why) = component_block(&n.to_string_lossy()) {
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
        let real_parent = parent.canonicalize().map_err(|_| Denied::Missing)?;
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
        let Ok(real) = p.canonicalize() else { return Some("does not exist".into()) };
        if !real.is_dir() {
            return Some("not a folder".into());
        }
        for q in [p, real.as_path()] {
            if let Some(why) = self.blocked(q, false) {
                return Some(format!("protected: {why}"));
            }
        }
        let under_volume = real.strip_prefix("/Volumes").map(|r| r.components().count() >= 2).unwrap_or(false);
        if !(real.starts_with(&self.home) || under_volume) {
            return Some("outside your home folder".into());
        }
        None
    }

    /// Home itself or one of the folders that are never cleaned as a whole (Desktop, Documents, Library, ...).
    pub fn is_protected_root(&self, p: &Path) -> bool {
        p == self.home || exact_blocks(&self.home).iter().any(|(b, _)| b == p)
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
                let under_volume = real.strip_prefix("/Volumes").map(|r| r.components().count() >= 2).unwrap_or(false);
                if !(real.starts_with(&self.home) || under_volume) {
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

    /// A throw-away "home" with the folders the blocklist knows about.
    fn fixture() -> (TempDir, Safety) {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().canonicalize().unwrap().join("home");
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

    #[test]
    fn symlinked_ancestors_cannot_smuggle_a_blocked_target() {
        let (_t, s) = fixture();
        let h = s.home().to_path_buf();
        // ~/Projects/link -> ~/.ssh : picking ~/Projects/link/id_ed25519 must resolve to .ssh and be blocked.
        std::os::unix::fs::symlink(h.join(".ssh"), h.join("Projects/link")).unwrap();
        assert!(matches!(s.check(&h.join("Projects/link/id_ed25519"), &Allowed::UserPick), Err(Denied::Blocked(_))));
        // A rule path that only matches lexically (Caches/evil -> Documents) is refused for rules.
        std::os::unix::fs::symlink(h.join("Documents"), h.join("Library/Caches/evil")).unwrap();
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
