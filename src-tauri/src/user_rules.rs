//! Custom rules made by the user. They only ever move things to the Trash, and every check below runs in
//! Rust: the UI is not trusted to have validated anything.

use crate::locate::{self, Locations};
use crate::rules::{Kind, Method, Risk, Rule, CATEGORIES, SCHEMA_VERSION};
use crate::safety::Safety;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const MAX_PATTERNS: usize = 20;
pub const MAX_MATCHES: usize = 5000;

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct Draft {
    /// `user.<slug>`; empty for a new rule.
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "custom")]
    pub category: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "review")]
    pub risk: Risk,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub busy: Vec<String>,
    /// Leave items alone until nothing in them has changed for this many days.
    #[serde(default)]
    pub min_age_days: Option<u32>,
}

fn custom() -> String {
    "Custom".into()
}
fn review() -> Risk {
    Risk::Review
}

impl Draft {
    pub fn to_rule(&self) -> Rule {
        Rule {
            id: self.id.clone(),
            name: self.name.trim().to_string(),
            category: self.category.clone(),
            kind: Kind::Path,
            paths: self.paths.clone(),
            exclude: self.exclude.clone(),
            detect: None,
            risk: self.risk,
            description: if self.description.trim().is_empty() { "Custom rule you added. Matching items move to the Trash.".into() } else { self.description.trim().to_string() },
            regenerates: "Unknown: this is a custom rule. Restore from the Trash if you need it back.".into(),
            method: Method::Trash,
            tool: None,
            busy: self.busy.clone(),
            opt_in: false,
            needs_admin: false,
            dynamic: None,
            dir_name: None,
            markers: Vec::new(),
            stale_days: None,
            min_age_days: self.min_age_days.filter(|d| *d > 0),
            app: None,
            custom: true,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default = "one")]
    schema_version: u32,
    #[serde(default, rename = "rule")]
    rules: Vec<Draft>,
}
fn one() -> u32 {
    SCHEMA_VERSION
}

pub fn parse(text: &str) -> Result<Vec<Draft>, String> {
    let f: File = toml::from_str(text).map_err(|e| format!("not a Deskveet rules file: {e}"))?;
    Ok(f.rules)
}

pub fn load(path: &Path) -> Vec<Draft> {
    fs::read_to_string(path).ok().and_then(|t| parse(&t).ok()).unwrap_or_default()
}

pub fn to_toml(drafts: &[Draft]) -> String {
    let body = toml::to_string(&File { schema_version: SCHEMA_VERSION, rules: drafts.to_vec() }).unwrap_or_default();
    format!("# Deskveet custom rules. Items they match always move to the Trash.\n{body}")
}

pub fn save(path: &Path, drafts: &[Draft]) -> io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, to_toml(drafts))?;
    fs::rename(tmp, path)
}

pub fn slug(name: &str) -> String {
    let mut s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_');
    if s.is_empty() { "rule".into() } else { s.chars().take(40).collect() }
}

/// A free `user.<slug>` id.
pub fn new_id(name: &str, taken: &HashSet<String>) -> String {
    let base = format!("user.{}", slug(name));
    if !taken.contains(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}_{n}")).find(|id| !taken.contains(id)).unwrap()
}

/// The pattern with `{location}` and `~` replaced, unescaped (for reasoning about its folders).
fn raw(pattern: &str, locs: &Locations, home: &Path) -> Option<PathBuf> {
    let mut s = pattern.to_string();
    for t in locate::tokens(pattern) {
        s = s.replacen(&format!("{{{t}}}"), &locs.get(t)?.path.as_ref()?.to_string_lossy(), 1);
    }
    Some(match s.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(s),
    })
}

fn is_wild(c: &str) -> bool {
    c.contains(['*', '?', '['])
}

/// Everything about a draft that can be checked without touching the disk.
pub fn check_shape(d: &Draft, locs: &Locations, safety: &Safety) -> Result<(), String> {
    if d.name.trim().is_empty() {
        return Err("Give the rule a name.".into());
    }
    if !CATEGORIES.contains(&d.category.as_str()) {
        return Err("Pick one of the categories.".into());
    }
    let pats: Vec<&String> = d.paths.iter().filter(|p| !p.trim().is_empty()).collect();
    if pats.is_empty() {
        return Err("Add at least one path.".into());
    }
    if pats.len() > MAX_PATTERNS {
        return Err(format!("Use at most {MAX_PATTERNS} paths."));
    }
    let home = safety.home();
    for p in pats.iter().map(|p| p.trim()).chain(d.exclude.iter().map(|s| s.trim()).filter(|s| !s.is_empty())) {
        if p.contains("**") {
            return Err(format!("{p}: recursive ** patterns aren't allowed. Name the folders level by level."));
        }
        if !(p.starts_with("~/") || p.starts_with('/') || p.starts_with('{')) || p.contains("..") {
            return Err(format!("{p}: start with ~/, / or a {{location}}, and don't use .."));
        }
        if let Some(t) = locate::tokens(p).into_iter().find(|t| !locs.contains_key(*t)) {
            return Err(format!("{p}: unknown location {{{t}}}"));
        }
    }
    for p in pats.iter().map(|p| p.trim()) {
        let Some(path) = raw(p, locs, home) else { continue }; // location not on this Mac: matches nothing
        let comps: Vec<Component> = path.components().collect();
        let first_wild = comps.iter().position(|c| is_wild(&c.as_os_str().to_string_lossy()));
        let prefix: PathBuf = comps[..first_wild.unwrap_or(comps.len())].iter().collect();
        let under_volume = prefix.strip_prefix("/Volumes").map(|r| r.components().count() >= 2).unwrap_or(false);
        if !(prefix.starts_with(home) && prefix != home || under_volume) {
            return Err(format!("{p}: custom rules can only look inside your home folder (not at it) or on an external disk."));
        }
        if let Some(i) = first_wild {
            let wild = comps[i].as_os_str().to_string_lossy();
            let literal = wild.chars().filter(|c| !matches!(c, '*' | '?' | '[' | ']')).count();
            if safety.is_protected_root(&prefix) && literal < 2 {
                return Err(format!("{p}: too broad. \u{201c}{wild}\u{201d} directly inside {} would match everything there; add a name or extension such as *.dmg.", prefix.display()));
            }
        } else if safety.is_protected_root(&prefix) {
            return Err(format!("{p}: that folder can never be cleaned as a whole."));
        }
        if let Some(why) = safety.protected_reason(&prefix) {
            if !safety.is_protected_root(&prefix) {
                return Err(format!("{p}: inside a protected location ({why})."));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx() -> (tempfile::TempDir, Safety, Locations) {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().canonicalize().unwrap().join("home");
        for d in ["Downloads", "Documents", "Library/Caches/com.example", "Projects/app/.git", "dev/cache"] {
            fs::create_dir_all(h.join(d)).unwrap();
        }
        let s = Safety::new(h);
        (t, s, Locations::new())
    }

    fn d(paths: &[&str]) -> Draft {
        Draft { id: String::new(), name: "Test".into(), category: "Custom".into(), paths: paths.iter().map(|s| s.to_string()).collect(), exclude: vec![], risk: Risk::Review, description: String::new(), busy: vec![], min_age_days: None }
    }

    #[test]
    fn broad_or_dangerous_patterns_are_refused() {
        let (_t, s, l) = fx();
        for (p, why) in [
            ("~/*", "everything in home"),
            ("~", "home itself"),
            ("~/Documents/*", "everything in Documents"),
            ("~/Library/*", "everything in Library"),
            ("~/Documents", "a protected root"),
            ("~/Downloads/**", "recursive"),
            ("/System/Library/Caches/*", "system"),
            ("/tmp/x/*", "outside home"),
            ("~/.ssh/*", "ssh keys"),
            ("~/Library/Mail/*", "mail"),
            ("relative/*", "relative"),
            ("~/a/../b", "dot-dot"),
            ("{nowhere}/x", "unknown location"),
        ] {
            assert!(check_shape(&d(&[p]), &l, &s).is_err(), "{why}: {p}");
        }
    }

    #[test]
    fn specific_patterns_are_fine() {
        let (_t, s, l) = fx();
        for p in ["~/Downloads/*.dmg", "~/Library/Caches/com.example/*", "~/dev/cache/*", "~/Projects/*/build", "~/Downloads/old-*"] {
            assert!(check_shape(&d(&[p]), &l, &s).is_ok(), "{p}: {:?}", check_shape(&d(&[p]), &l, &s));
        }
        assert!(check_shape(&Draft { name: " ".into(), ..d(&["~/dev/cache/*"]) }, &l, &s).is_err(), "needs a name");
        assert!(check_shape(&d(&[]), &l, &s).is_err(), "needs a path");
        assert!(check_shape(&Draft { category: "Nope".into(), ..d(&["~/dev/cache/*"]) }, &l, &s).is_err());
    }

    #[test]
    fn custom_rules_always_move_to_trash_and_round_trip() {
        let mut a = d(&["~/dev/cache/*"]);
        a.id = "user.test".into();
        let r = a.to_rule();
        assert_eq!((r.method, r.kind, r.custom, r.tool.is_none()), (Method::Trash, Kind::Path, true, true));
        let text = to_toml(&[a.clone()]);
        assert_eq!(parse(&text).unwrap(), vec![a]);
        // a file that tries to sneak in a tool command is read as a plain Trash rule
        let evil = "schema_version = 1\n[[rule]]\nname = \"x\"\npaths = [\"~/dev/cache/*\"]\nmethod = \"tool\"\ntool = [\"rm\", \"-rf\", \"/\"]\n";
        let parsed = parse(evil).unwrap();
        assert_eq!(parsed[0].to_rule().method, Method::Trash);
        assert!(parse("not toml at all [[[").is_err());
    }

    #[test]
    fn ids_are_slugs_and_never_collide() {
        let taken: HashSet<String> = ["user.old_dmgs".to_string()].into();
        assert_eq!(new_id("Old DMGs!", &taken), "user.old_dmgs_2");
        assert_eq!(new_id("  ", &HashSet::new()), "user.rule");
    }
}
