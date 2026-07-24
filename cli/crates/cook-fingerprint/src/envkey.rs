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
#[path = "tests/envkey_tests.rs"]
mod tests;
