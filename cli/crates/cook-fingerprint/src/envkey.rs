//! Per-step env contribution to the cache key, with a two-layer denylist.
//!
//! D1: Cook-shipped baseline (`baseline()`) — universal noisy env.
//! D2: `.cook/cloud.toml [cache] ignore_env` extensions (`extend_with`).
//! Layer 2 inference (the consulted-env capture) is in cook-luagen/cook-register.

use std::collections::{BTreeMap, HashSet};

pub struct EnvDenylist {
    /// Exact-match names.
    names: HashSet<String>,
    /// Glob patterns like "XDG_*", "GITHUB_*". Compiled once at construction.
    globs: Vec<glob::Pattern>,
}

impl EnvDenylist {
    /// D1: Cook-shipped baseline. See spec Appendix A for the full list.
    pub fn baseline() -> Self {
        const EXACT: &[&str] = &[
            "HOME", "USER", "LOGNAME", "SHELL", "PATH", "PWD", "OLDPWD", "MAIL", "HOSTNAME",
            "TERM", "TERMINFO", "COLORTERM",
            "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY",
            "SSH_AUTH_SOCK", "SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY",
            "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_BUS_TYPE", "DBUS_STARTER_ADDRESS",
            "EDITOR", "VISUAL", "PAGER", "BROWSER",
            "TMPDIR", "TMP", "TEMP",
            "HISTFILE", "HISTSIZE", "HISTCONTROL",
            "SHLVL", "PS1", "PS2", "PS3", "PS4",
            "CI",
        ];
        const GLOBS: &[&str] = &[
            "XDG_*",
            "GITHUB_*", "RUNNER_*",
            "GITLAB_CI_*",
            "BUILDKITE_*",
            "CIRCLE_*",
            "TRAVIS_*",
            "JENKINS_*",
            "TEAMCITY_*",
            "DRONE_*",
        ];

        let names: HashSet<String> = EXACT.iter().map(|s| (*s).to_string()).collect();
        let globs: Vec<glob::Pattern> = GLOBS
            .iter()
            .map(|p| glob::Pattern::new(p).expect("baseline glob compiles"))
            .collect();
        Self { names, globs }
    }

    /// Extend with project-level (.cook/cloud.toml) additions. Idempotent on overlap.
    pub fn extend_with(&mut self, additions: &[String]) {
        for a in additions {
            if a.contains('*') || a.contains('?') {
                if let Ok(p) = glob::Pattern::new(a) {
                    self.globs.push(p);
                }
            } else {
                self.names.insert(a.clone());
            }
        }
    }

    pub fn is_ignored(&self, key: &str) -> bool {
        if self.names.contains(key) {
            return true;
        }
        self.globs.iter().any(|p| p.matches(key))
    }
}

/// Compute the env contribution hash for a step.
///
/// `consulted` is the BTreeMap of (name → value) pairs that the step's
/// command consulted (per Layer 2 inference). The denylist filters
/// names whose values must not contribute to the cache key.
///
/// xxh3_64 because this is a local-cache hash; the cloud-key SHA-256
/// composition reads this field directly.
pub fn env_contribution(consulted: &BTreeMap<String, String>, denylist: &EnvDenylist) -> u64 {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    for (k, v) in consulted {
        if denylist.is_ignored(k) {
            continue;
        }
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"\n");
    }
    hasher.digest()
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn baseline_excludes_HOME() {
        let d = EnvDenylist::baseline();
        assert!(d.is_ignored("HOME"));
    }

    #[test]
    fn baseline_excludes_PATH() {
        let d = EnvDenylist::baseline();
        assert!(d.is_ignored("PATH"));
    }

    #[test]
    fn baseline_excludes_XDG_glob() {
        let d = EnvDenylist::baseline();
        assert!(d.is_ignored("XDG_RUNTIME_DIR"));
        assert!(d.is_ignored("XDG_CONFIG_HOME"));
    }

    #[test]
    fn baseline_excludes_GITHUB_glob() {
        let d = EnvDenylist::baseline();
        assert!(d.is_ignored("GITHUB_TOKEN"));
        assert!(d.is_ignored("GITHUB_ACTIONS"));
    }

    #[test]
    fn baseline_does_not_exclude_CFLAGS() {
        let d = EnvDenylist::baseline();
        assert!(!d.is_ignored("CFLAGS"));
        assert!(!d.is_ignored("CXXFLAGS"));
        assert!(!d.is_ignored("CPATH"));
    }

    #[test]
    fn baseline_does_not_exclude_LANG_or_LC() {
        let d = EnvDenylist::baseline();
        assert!(!d.is_ignored("LANG"));
        assert!(!d.is_ignored("LC_ALL"));
        assert!(!d.is_ignored("LC_CTYPE"));
        assert!(!d.is_ignored("TZ"));
        assert!(!d.is_ignored("SOURCE_DATE_EPOCH"));
    }

    #[test]
    fn extend_with_adds_user_names() {
        let mut d = EnvDenylist::baseline();
        d.extend_with(&["MY_API_TOKEN".to_string(), "MY_SECRET".to_string()]);
        assert!(d.is_ignored("MY_API_TOKEN"));
        assert!(d.is_ignored("MY_SECRET"));
        assert!(d.is_ignored("HOME"), "baseline still applies");
    }

    #[test]
    fn extend_with_overlap_is_idempotent() {
        let mut d = EnvDenylist::baseline();
        d.extend_with(&["HOME".to_string()]);
        assert!(d.is_ignored("HOME"));
    }

    #[test]
    fn env_contribution_empty_consulted_is_constant() {
        let d = EnvDenylist::baseline();
        let consulted = BTreeMap::new();
        let h1 = env_contribution(&consulted, &d);
        let h2 = env_contribution(&consulted, &d);
        assert_eq!(h1, h2);
    }

    #[test]
    fn env_contribution_filtered_keys_excluded() {
        let d = EnvDenylist::baseline();
        let mut a = BTreeMap::new();
        a.insert("CFLAGS".to_string(), "-O2".to_string());
        let mut b = a.clone();
        b.insert("HOME".to_string(), "/home/alice".to_string());
        let h_a = env_contribution(&a, &d);
        let h_b = env_contribution(&b, &d);
        assert_eq!(h_a, h_b, "denylisted HOME must not contribute");
    }

    #[test]
    fn env_contribution_kept_keys_included() {
        let d = EnvDenylist::baseline();
        let mut a = BTreeMap::new();
        a.insert("CFLAGS".to_string(), "-O2".to_string());
        let mut b = BTreeMap::new();
        b.insert("CFLAGS".to_string(), "-O3".to_string());
        let h_a = env_contribution(&a, &d);
        let h_b = env_contribution(&b, &d);
        assert_ne!(h_a, h_b, "CFLAGS value change must change hash");
    }

    #[test]
    fn env_contribution_value_change_changes_hash() {
        let d = EnvDenylist::baseline();
        let mut a = BTreeMap::new();
        a.insert("MYVAR".to_string(), "v1".to_string());
        let mut b = BTreeMap::new();
        b.insert("MYVAR".to_string(), "v2".to_string());
        assert_ne!(env_contribution(&a, &d), env_contribution(&b, &d));
    }

    #[test]
    fn env_contribution_iteration_order_independent() {
        let d = EnvDenylist::baseline();
        let mut a = BTreeMap::new();
        a.insert("Z".to_string(), "1".to_string());
        a.insert("A".to_string(), "2".to_string());
        let mut b = BTreeMap::new();
        b.insert("A".to_string(), "2".to_string());
        b.insert("Z".to_string(), "1".to_string());
        assert_eq!(env_contribution(&a, &d), env_contribution(&b, &d));
    }
}
