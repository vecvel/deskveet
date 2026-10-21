//! Where tools really keep their data on *this* Mac.
//!
//! A `[[location]]` in the rules file names a folder that tools let users move (Android SDK, Gradle, Cargo,
//! pub, npm, Go, Homebrew, Xcode DerivedData, ...). Rules refer to it as `{id}` in their paths. Each location
//! is resolved once per session, in order: ask the tool, then environment variables (the app's own and the
//! user's login shell's, because apps opened from the Dock don't inherit shell variables), then the default.
//! A candidate that is missing, or points at home, a protected folder or a system path, is skipped and the
//! reason is kept so the UI can show it.

use crate::safety::{expand_tilde, Safety};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct LocationDef {
    pub id: String,
    /// Variables to try, in order. `VAR/sub/dir` means "$VAR joined with sub/dir".
    #[serde(default)]
    pub env: Vec<String>,
    /// A read-only command whose first output line is the folder (for example `go env GOCACHE`).
    pub cmd: Option<Vec<String>>,
    /// Used when nothing else gives a usable folder. `~/...` or absolute.
    pub default: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Resolved {
    pub id: String,
    pub path: Option<PathBuf>,
    /// "default", "$GRADLE_USER_HOME", "`go env GOCACHE`".
    pub source: String,
    /// Candidates that were refused, with the reason ("$GRADLE_USER_HOME points to ~/Documents").
    pub skipped: Vec<String>,
}

pub type Locations = HashMap<String, Resolved>;

fn candidate_from_env(spec: &str, env: &HashMap<String, String>) -> Option<(String, PathBuf)> {
    let (var, sub) = spec.split_once('/').map_or((spec, ""), |(v, s)| (v, s));
    let val = env.get(var).map(|v| v.trim()).filter(|v| !v.is_empty())?;
    let mut p = PathBuf::from(val);
    if !sub.is_empty() {
        p = p.join(sub);
    }
    Some((format!("${var}"), p))
}

fn tilde(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if p == "~" => home.to_path_buf(),
        None => PathBuf::from(p),
    }
}

/// Resolve every location. `query` runs a read-only tool command and returns stdout (None on failure).
pub fn resolve(defs: &[LocationDef], env: &HashMap<String, String>, query: &(dyn Fn(&[String]) -> Option<String> + Sync), safety: &Safety) -> Locations {
    let home = safety.home().to_path_buf();
    let one = |d: &LocationDef| -> Resolved {
        let mut skipped = Vec::new();
        let mut cands: Vec<(String, PathBuf)> = Vec::new();
        if let Some(cmd) = &d.cmd {
            if let Some(out) = query(cmd) {
                if let Some(line) = out.lines().map(str::trim).find(|l| !l.is_empty()) {
                    cands.push((format!("`{}`", cmd.join(" ")), tilde(line, &home)));
                }
            }
        }
        for spec in &d.env {
            if let Some((src, p)) = candidate_from_env(spec, env) {
                cands.push((src, tilde(&p.to_string_lossy(), &home)));
            }
        }
        if let Some(def) = &d.default {
            cands.push(("default".into(), tilde(def, &home)));
        }
        for (src, p) in cands {
            match safety.location_problem(&p) {
                None => return Resolved { id: d.id.clone(), path: Some(p.canonicalize().unwrap_or(p)), source: src, skipped },
                // A folder that simply doesn't exist yet is normal (tool installed, nothing cached); only
                // refusals for safety are worth telling the user about.
                Some(why) if src != "default" && why != "does not exist" => {
                    skipped.push(format!("{src} points to {} ({why}), ignored", p.display()));
                }
                Some(_) => {}
            }
        }
        Resolved { id: d.id.clone(), path: None, source: "not found".into(), skipped }
    };
    // Tool queries can each take a moment (npm, brew): run them side by side.
    std::thread::scope(|s| {
        let handles: Vec<_> = defs.iter().map(|d| s.spawn(move || one(d))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).map(|r| (r.id.clone(), r)).collect()
    })
}

/// Location ids used by a pattern, e.g. `{gradle_home}/caches/*` -> ["gradle_home"].
pub fn tokens(pattern: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = pattern;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start + 1..].find('}') else { break };
        out.push(&rest[start + 1..start + 1 + len]);
        rest = &rest[start + 2 + len..];
    }
    out
}

/// Replace `{id}` tokens with resolved folders (glob-escaped). `None` when a token has no usable folder.
pub fn expand(pattern: &str, locs: &Locations, home: &Path) -> Option<String> {
    let mut s = pattern.to_string();
    for t in tokens(pattern) {
        let path = locs.get(t)?.path.as_ref()?;
        s = s.replacen(&format!("{{{t}}}"), &glob::Pattern::escape(&path.to_string_lossy()), 1);
    }
    Some(expand_tilde(&s, home))
}

/// Locations resolved to their defaults only (tests, and the rules self-check).
pub fn defaults(defs: &[LocationDef], home: &Path) -> Locations {
    defs.iter()
        .map(|d| (d.id.clone(), Resolved { id: d.id.clone(), path: d.default.as_deref().map(|p| tilde(p, home)), source: "default".into(), skipped: vec![] }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn home() -> (tempfile::TempDir, Safety) {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().canonicalize().unwrap().join("home");
        for d in ["Documents", "Desktop", ".gradle", "custom/gradle", "Library/Caches/go-build", "tools/go-cache", "sdk/android/avd", "Library"] {
            fs::create_dir_all(h.join(d)).unwrap();
        }
        let s = Safety::new(h);
        (t, s)
    }

    fn def(id: &str, env: &[&str], cmd: Option<&[&str]>, default: &str) -> LocationDef {
        LocationDef { id: id.into(), env: env.iter().map(|s| s.to_string()).collect(), cmd: cmd.map(|c| c.iter().map(|s| s.to_string()).collect()), default: Some(default.into()) }
    }

    fn env(pairs: &[(&str, String)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn order_is_tool_then_env_then_default() {
        let (_t, s) = home();
        let h = s.home().to_path_buf();
        let d = def("go_cache", &["GOCACHE"], Some(&["go", "env", "GOCACHE"]), "~/Library/Caches/go-build");
        let tool_dir = h.join("tools/go-cache").to_string_lossy().into_owned();
        let q_tool = |_: &[String]| Some(format!("{tool_dir}\n"));
        let r = &resolve(&[d.clone()], &env(&[("GOCACHE", h.join("custom/gradle").to_string_lossy().into())]), &q_tool, &s)["go_cache"];
        assert_eq!((r.path.as_deref(), r.source.as_str()), (Some(h.join("tools/go-cache").as_path()), "`go env GOCACHE`"));
        let no_tool = |_: &[String]| None;
        let r = &resolve(&[d.clone()], &env(&[("GOCACHE", h.join("custom/gradle").to_string_lossy().into())]), &no_tool, &s)["go_cache"];
        assert_eq!(r.source, "$GOCACHE");
        let r = &resolve(&[d], &HashMap::new(), &no_tool, &s)["go_cache"];
        assert_eq!((r.path.as_deref(), r.source.as_str()), (Some(h.join("Library/Caches/go-build").as_path()), "default"));
    }

    #[test]
    fn dangerous_or_missing_values_are_skipped_with_a_reason() {
        let (_t, s) = home();
        let h = s.home().to_path_buf();
        let d = def("gradle_home", &["GRADLE_USER_HOME"], None, "~/.gradle");
        let none = |_: &[String]| None;
        let missing = h.join("does/not/exist");
        let r = &resolve(&[d.clone()], &env(&[("GRADLE_USER_HOME", missing.to_string_lossy().into())]), &none, &s)["gradle_home"];
        assert!(r.source == "default" && r.skipped.is_empty(), "a missing folder falls through quietly");
        for bad in [h.clone(), h.join("Documents"), h.join("Desktop"), h.join("Library"), PathBuf::from("/System/Library"), PathBuf::from("/")] {
            let r = &resolve(&[d.clone()], &env(&[("GRADLE_USER_HOME", bad.to_string_lossy().into())]), &none, &s)["gradle_home"];
            assert_eq!(r.source, "default", "{bad:?} must not be used");
            assert_eq!(r.skipped.len(), 1, "{bad:?}");
            assert!(r.skipped[0].contains("$GRADLE_USER_HOME"), "{:?}", r.skipped);
        }
        // a sane custom value wins over the default
        let r = &resolve(&[d.clone()], &env(&[("GRADLE_USER_HOME", h.join("custom/gradle").to_string_lossy().into())]), &none, &s)["gradle_home"];
        assert_eq!(r.path.as_deref(), Some(h.join("custom/gradle").as_path()));
        // ~ in a value is expanded
        let r = &resolve(&[d], &env(&[("GRADLE_USER_HOME", "~/custom/gradle".into())]), &none, &s)["gradle_home"];
        assert_eq!(r.path.as_deref(), Some(h.join("custom/gradle").as_path()));
    }

    #[test]
    fn env_specs_can_append_a_sub_folder_and_nothing_found_is_reported() {
        let (_t, s) = home();
        let h = s.home().to_path_buf();
        let none = |_: &[String]| None;
        let d = def("android_avd", &["ANDROID_AVD_HOME", "ANDROID_USER_HOME/avd"], None, "~/.android/avd");
        let r = &resolve(&[d.clone()], &env(&[("ANDROID_USER_HOME", h.join("sdk/android").to_string_lossy().into())]), &none, &s)["android_avd"];
        assert_eq!((r.path.as_deref(), r.source.as_str()), (Some(h.join("sdk/android/avd").as_path()), "$ANDROID_USER_HOME"));
        let r = &resolve(&[d], &HashMap::new(), &none, &s)["android_avd"];
        assert!(r.path.is_none() && r.source == "not found", "default ~/.android/avd does not exist here");
    }

    #[test]
    fn tokens_expand_with_escaping_and_unresolved_tokens_drop_the_pattern() {
        let (_t, s) = home();
        let h = s.home().to_path_buf();
        let mut locs = Locations::new();
        locs.insert("g".into(), Resolved { id: "g".into(), path: Some(h.join("we[ir]d")), source: "default".into(), skipped: vec![] });
        locs.insert("missing".into(), Resolved { id: "missing".into(), path: None, source: "not found".into(), skipped: vec![] });
        assert_eq!(tokens("{g}/caches/*"), vec!["g"]);
        let e = expand("{g}/caches/*", &locs, &h).unwrap();
        assert!(e.ends_with("we[[]ir[]]d/caches/*"), "{e}");
        assert!(expand("{missing}/x", &locs, &h).is_none());
        assert!(expand("{nope}/x", &locs, &h).is_none());
        assert_eq!(expand("~/Library/Caches/*", &locs, &h).unwrap(), format!("{}/Library/Caches/*", glob::Pattern::escape(&h.to_string_lossy())));
    }
}
