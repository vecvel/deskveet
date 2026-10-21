//! The rule registry: a versioned TOML file shipped inside the app (rules/macos.toml).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Highest schema this build reads. v2 adds `[[location]]` and `{location}` tokens in paths.
pub const SCHEMA_VERSION: u32 = 2;
pub const CATEGORIES: &[&str] = &["Developer", "Browsers", "App caches", "System", "Package managers", "Project build artifacts"];

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Safe,
    Review,
    Caution,
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    Trash,
    Tool,
    Erase,
}

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Path,
    Tool,
    Dynamic,
    Artifact,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub category: String,
    #[serde(default)]
    pub kind: Kind,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub detect: Option<String>,
    pub risk: Risk,
    pub description: String,
    pub regenerates: String,
    pub method: Method,
    pub tool: Option<Vec<String>>,
    #[serde(default)]
    pub busy: Vec<String>,
    #[serde(default)]
    pub opt_in: bool,
    #[serde(default)]
    pub needs_admin: bool,
    pub dynamic: Option<String>,
    pub dir_name: Option<String>,
    #[serde(default)]
    pub markers: Vec<String>,
    pub stale_days: Option<u32>,
}

impl Rule {
    /// Human-readable command for the preview, e.g. `brew cleanup -s`.
    pub fn tool_display(&self) -> Option<String> {
        self.tool.as_ref().map(|t| t.join(" "))
    }
}

#[derive(Deserialize, Debug)]
pub struct Registry {
    pub schema_version: u32,
    pub rules_version: String,
    /// Folders tools let users move; rules refer to them as `{id}`.
    #[serde(default, rename = "location")]
    pub locations: Vec<crate::locate::LocationDef>,
    #[serde(rename = "rule")]
    pub rules: Vec<Rule>,
}

const DYNAMIC_KINDS: &[&str] = &["simctl_devices", "simctl_runtimes", "trash"];

impl Registry {
    pub fn parse(text: &str) -> Result<Registry, String> {
        let reg: Registry = toml::from_str(text).map_err(|e| format!("rules file: {e}"))?;
        reg.validate()?;
        Ok(reg)
    }

    pub fn macos() -> Result<Registry, String> {
        Registry::parse(include_str!("../../rules/macos.toml"))
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(1..=SCHEMA_VERSION).contains(&self.schema_version) {
            return Err(format!("unsupported rules schema {} (this build understands 1 to {SCHEMA_VERSION})", self.schema_version));
        }
        let mut loc_ids = HashSet::new();
        for l in &self.locations {
            if l.id.is_empty() || !l.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                return Err(format!("location `{}`: id must be lowercase letters, digits or _", l.id));
            }
            if !loc_ids.insert(l.id.as_str()) {
                return Err(format!("location `{}`: duplicate id", l.id));
            }
            if l.default.as_deref().is_some_and(|d| !(d.starts_with("~/") || d.starts_with('/')) || d.contains("..")) {
                return Err(format!("location `{}`: default must be absolute or start with ~/", l.id));
            }
            if l.env.is_empty() && l.cmd.is_none() && l.default.is_none() {
                return Err(format!("location `{}`: needs env, cmd or default", l.id));
            }
        }
        let mut seen = HashSet::new();
        for r in &self.rules {
            let id = &r.id;
            let bad = |m: &str| Err(format!("rule `{id}`: {m}"));
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_') {
                return bad("id must be lowercase dotted ascii");
            }
            if !seen.insert(id.clone()) {
                return bad("duplicate id");
            }
            if r.name.trim().is_empty() {
                return bad("empty name");
            }
            if !CATEGORIES.contains(&r.category.as_str()) {
                return bad("unknown category");
            }
            if r.description.trim().len() < 12 {
                return bad("description (what happens if you delete it) is missing");
            }
            if r.regenerates.trim().is_empty() {
                return bad("regenerates is missing");
            }
            if r.method == Method::Tool && r.tool.as_ref().map_or(true, |t| t.is_empty()) {
                return bad("method = tool needs a tool command");
            }
            match r.kind {
                Kind::Path if r.paths.is_empty() => return bad("path rule without paths"),
                Kind::Tool if r.method != Method::Tool => return bad("tool rule must use method = tool"),
                Kind::Dynamic if !r.dynamic.as_deref().is_some_and(|d| DYNAMIC_KINDS.contains(&d)) => return bad("unknown dynamic source"),
                Kind::Artifact if r.dir_name.as_deref().map_or(true, str::is_empty) => return bad("artifact rule needs dir_name"),
                _ => {}
            }
            for p in r.paths.iter().chain(&r.exclude) {
                let toks = crate::locate::tokens(p);
                if let Some(t) = toks.iter().find(|t| !loc_ids.contains(**t)) {
                    return bad(&format!("unknown location {{{t}}}"));
                }
                let starts_ok = p.starts_with("~/") || p.starts_with('/') || toks.first().is_some_and(|t| p.starts_with(&format!("{{{t}}}")));
                if !starts_ok {
                    return bad("paths must be absolute, start with ~/ or with a {location}");
                }
                if p.contains("..") {
                    return bad("paths must not contain ..");
                }
            }
            if let Some(t) = &r.tool {
                for a in t {
                    if a.contains('<') && !matches!(a.as_str(), "<udid>" | "<id>") {
                        return bad("unknown tool placeholder");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_registry_parses_and_validates() {
        let reg = Registry::macos().expect("rules/macos.toml must be valid");
        assert!(reg.rules.len() >= 30, "{} rules", reg.rules.len());
        assert!(!reg.rules_version.is_empty());
    }

    #[test]
    fn every_rule_has_risk_description_and_method() {
        // `risk` and `method` are required serde fields (a missing one fails to parse);
        // description / regenerates are checked by validate(). Re-assert the contract explicitly.
        let reg = Registry::macos().unwrap();
        for r in &reg.rules {
            assert!(r.description.len() >= 12, "{}", r.id);
            assert!(!r.regenerates.is_empty(), "{}", r.id);
            match r.method {
                Method::Tool => assert!(r.tool.as_ref().is_some_and(|t| !t.is_empty()), "{}", r.id),
                Method::Trash | Method::Erase => {}
            }
        }
    }

    #[test]
    fn spec_rules_are_present() {
        let reg = Registry::macos().unwrap();
        for id in [
            "xcode.derived_data", "xcode.archives", "xcode.ios_device_support", "xcode.xctest_devices", "simulators.devices",
            "simulators.runtimes", "android.avd", "android.system_images", "android.emulator", "android.gradle_caches",
            "android.gradle_wrappers", "flutter.pub_cache", "node.npm_cache", "node.pnpm_store", "node.yarn_cache",
            "artifact.node_modules", "rust.cargo_registry", "artifact.rust_target", "go.build_cache", "python.pip_cache",
            "artifact.pycache", "brew.cache", "cocoapods.cache", "browser.chrome", "browser.brave", "browser.safari",
            "browser.firefox", "appcache.library_caches", "system.user_logs", "system.updater_caches", "system.trash", "docker.prune",
        ] {
            assert!(reg.get(id).is_some(), "missing rule {id}");
        }
        assert_eq!(reg.get("xcode.archives").unwrap().risk, Risk::Caution);
        assert_eq!(reg.get("flutter.pub_cache").unwrap().risk, Risk::Review);
        assert!(reg.get("docker.prune").unwrap().opt_in);
    }

    #[test]
    fn invalid_rules_are_rejected() {
        let base = |extra: &str| format!("schema_version = 1\nrules_version = \"x\"\n[[rule]]\nid = \"a.b\"\nname = \"A\"\ncategory = \"System\"\npaths = [\"~/x\"]\nrisk = \"safe\"\ndescription = \"Deletes the thing for testing.\"\nregenerates = \"yes\"\nmethod = \"trash\"\n{extra}");
        assert!(Registry::parse(&base("")).is_ok());
        assert!(Registry::parse(&base("").replace("method = \"trash\"\n", "")).is_err(), "method is required");
        assert!(Registry::parse(&base("").replace("risk = \"safe\"\n", "")).is_err(), "risk is required");
        assert!(Registry::parse(&base("").replace("Deletes the thing for testing.", "")).is_err(), "description is required");
        assert!(Registry::parse(&base("").replace("\"System\"", "\"Nope\"")).is_err());
        assert!(Registry::parse(&base("").replace("\"~/x\"", "\"relative/x\"")).is_err());
        assert!(Registry::parse(&base("").replace("\"~/x\"", "\"~/../x\"")).is_err());
        assert!(Registry::parse(&base("").replace("schema_version = 1", "schema_version = 9")).is_err());
        assert!(Registry::parse(&base("").replace("\"~/x\"", "\"{nowhere}/x\"")).is_err(), "unknown location");
        let one = base("");
        let second = one.split_once("[[rule]]").unwrap().1;
        assert!(Registry::parse(&format!("{one}\n[[rule]]{second}")).is_err(), "duplicate id");
        assert!(Registry::parse(&base("").replace("method = \"trash\"", "method = \"tool\"")).is_err(), "tool without command");
    }

    #[test]
    fn schema_two_locations_are_validated() {
        let doc = |loc: &str, path: &str| format!("schema_version = 2\nrules_version = \"x\"\n{loc}\n[[rule]]\nid = \"a.b\"\nname = \"A\"\ncategory = \"System\"\npaths = [\"{path}\"]\nrisk = \"safe\"\ndescription = \"Deletes the thing for testing.\"\nregenerates = \"yes\"\nmethod = \"trash\"\n");
        let ok_loc = "[[location]]\nid = \"gradle_home\"\nenv = [\"GRADLE_USER_HOME\"]\ndefault = \"~/.gradle\"";
        assert!(Registry::parse(&doc(ok_loc, "{gradle_home}/caches/*")).is_ok());
        assert!(Registry::parse(&doc(ok_loc, "caches/{gradle_home}")).is_err(), "token must start the path");
        assert!(Registry::parse(&doc("[[location]]\nid = \"Bad-Id\"\ndefault = \"~/x\"", "~/x")).is_err());
        assert!(Registry::parse(&doc("[[location]]\nid = \"x\"\ndefault = \"relative\"", "~/x")).is_err());
        assert!(Registry::parse(&doc("[[location]]\nid = \"x\"", "~/x")).is_err(), "needs a source");
        let reg = Registry::macos().unwrap();
        assert_eq!(reg.schema_version, 2);
        assert!(reg.locations.iter().any(|l| l.id == "android_sdk"));
    }
}
