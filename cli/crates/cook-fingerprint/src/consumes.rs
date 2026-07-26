//! `consumes` — the glob allowlist narrowing which predecessor outputs fold
//! into a test unit's ready-time fingerprint (§17.4 step 1).
//!
//! A test unit's key folds every immediate-predecessor output. That is the
//! only shape available to it — unlike a cook unit, which declares its own
//! inputs and can record what it actually read via `discovered_inputs`, a
//! test has no way to say "of my dependency's `dist/`, I read the `.d.ts`
//! and nothing else". So one churny artifact class re-keys the check.
//! Sourcemaps are the flagship case: tsup and esbuild inline
//! `sourcesContent`, so a comment-only edit upstream rewrites
//! `index.mjs.map` while `index.mjs` stays byte-identical, and a downstream
//! typecheck loses its cached pass over a file it never opened.
//!
//! Matching follows gitignore convention, because that is the convention
//! every developer already has loaded:
//!
//!   - no `/` in the pattern → match the path's BASENAME, at any depth.
//!     `*.d.ts` covers `packages/core/dist/index.d.ts`.
//!   - a `/` in the pattern → match the whole project-root-relative path.
//!     `packages/core/dist/**/*.mjs` matches exactly that subtree.
//!
//! `*` never crosses a path separator (`literal_separator`); `**` does.
//!
//! **Narrowing a key is a correctness risk in the under-keying direction**,
//! which is why this module is deliberately conservative: an empty filter
//! means "fold everything", and [`ConsumesFilter::select`] returns the
//! unnarrowed set when a non-empty filter matches nothing. A declaration
//! that cannot be honoured must not quietly weaken a cache key — the
//! failure mode there is a stale pass replayed against changed inputs,
//! which is strictly worse than the over-invalidation the filter exists to
//! remove.

use globset::{Glob, GlobBuilder, GlobMatcher};

/// One compiled `consumes` pattern, tagged with what it matches against.
struct Rule {
    matcher: GlobMatcher,
    /// gitignore convention: a slash-less pattern matches the basename.
    basename_only: bool,
}

/// A compiled `consumes` allowlist. Empty means "no narrowing".
pub struct ConsumesFilter {
    rules: Vec<Rule>,
}

impl ConsumesFilter {
    /// Compile `patterns`. Returns the offending pattern and the parse error
    /// on failure, so callers can name it in a register-time diagnostic.
    pub fn compile(patterns: &[String]) -> Result<Self, (String, String)> {
        let mut rules = Vec::with_capacity(patterns.len());
        for p in patterns {
            let matcher = build(p).map_err(|e| (p.clone(), e))?;
            rules.push(Rule { matcher, basename_only: !p.contains('/') });
        }
        Ok(Self { rules })
    }

    /// True when no narrowing is declared — every candidate folds.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// True when `root_rel` (a project-root-relative, forward-slashed path)
    /// is named by at least one pattern.
    pub fn matches(&self, root_rel: &str) -> bool {
        let base = root_rel.rsplit('/').next().unwrap_or(root_rel);
        self.rules.iter().any(|r| {
            if r.basename_only {
                r.matcher.is_match(base)
            } else {
                r.matcher.is_match(root_rel)
            }
        })
    }

    /// Apply the filter to `candidates`, keyed by their root-relative paths.
    ///
    /// Returns every candidate when the filter is empty, and — deliberately —
    /// also when the filter matches nothing at all. See the module header: a
    /// filter that selects nothing would drop the dependency-content
    /// determinant entirely, letting a stale pass replay. Over-folding is the
    /// safe direction to fail in.
    pub fn select<'a, T, F>(&self, candidates: &'a [T], root_rel: F) -> Vec<&'a T>
    where
        F: Fn(&T) -> String,
    {
        if self.is_empty() {
            return candidates.iter().collect();
        }
        let kept: Vec<&T> =
            candidates.iter().filter(|c| self.matches(&root_rel(c))).collect();
        if kept.is_empty() && !candidates.is_empty() {
            return candidates.iter().collect();
        }
        kept
    }
}

/// Validate one pattern without keeping the compiled form — for register-time
/// diagnostics, so an unparseable glob is rejected where the author can see
/// it rather than silently matching nothing at ready time.
pub fn validate_pattern(pattern: &str) -> Result<(), String> {
    build(pattern).map(|_| ())
}

fn build(pattern: &str) -> Result<GlobMatcher, String> {
    if pattern.is_empty() {
        return Err("empty pattern".to_string());
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g: Glob| g.compile_matcher())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(pats: &[&str]) -> ConsumesFilter {
        let owned: Vec<String> = pats.iter().map(|s| s.to_string()).collect();
        ConsumesFilter::compile(&owned).expect("patterns compile")
    }

    #[test]
    fn slashless_pattern_matches_basename_at_any_depth() {
        let f = filter(&["*.d.ts"]);
        assert!(f.matches("packages/core/dist/index.d.ts"));
        assert!(f.matches("index.d.ts"));
        assert!(!f.matches("packages/core/dist/index.mjs"));
    }

    #[test]
    fn sourcemap_sidecars_are_excluded_by_an_extension_allowlist() {
        // The case the whole surface exists for: `.mjs` folds, `.mjs.map`
        // does not, so a comment-only upstream edit leaves the key alone.
        let f = filter(&["*.mjs", "*.d.ts"]);
        assert!(f.matches("pkgs/lib/dist/index.mjs"));
        assert!(!f.matches("pkgs/lib/dist/index.mjs.map"));
    }

    #[test]
    fn pattern_with_slash_matches_the_root_relative_path() {
        let f = filter(&["packages/core/dist/**/*.mjs"]);
        assert!(f.matches("packages/core/dist/esm/a.mjs"));
        assert!(f.matches("packages/core/dist/a.mjs"));
        assert!(!f.matches("packages/other/dist/a.mjs"));
    }

    #[test]
    fn star_does_not_cross_a_path_separator() {
        let f = filter(&["dist/*.mjs"]);
        assert!(f.matches("dist/a.mjs"));
        assert!(!f.matches("dist/esm/a.mjs"));
    }

    #[test]
    fn empty_filter_selects_everything() {
        let f = filter(&[]);
        assert!(f.is_empty());
        let items = vec!["a.mjs".to_string(), "a.mjs.map".to_string()];
        assert_eq!(f.select(&items, |s| s.clone()).len(), 2);
    }

    #[test]
    fn a_filter_matching_nothing_keeps_the_unnarrowed_set() {
        // Fail-safe toward over-invalidation. A typo'd pattern must not
        // silently drop the dependency-content determinant and let a stale
        // pass replay.
        let f = filter(&["*.wasm"]);
        let items = vec!["a.mjs".to_string(), "a.mjs.map".to_string()];
        assert_eq!(f.select(&items, |s| s.clone()).len(), 2);
    }

    #[test]
    fn a_partial_match_narrows_normally() {
        let f = filter(&["*.mjs"]);
        let items = vec!["a.mjs".to_string(), "a.mjs.map".to_string()];
        let kept = f.select(&items, |s| s.clone());
        assert_eq!(kept, vec![&"a.mjs".to_string()]);
    }

    #[test]
    fn empty_candidates_stay_empty() {
        // A dep that produced nothing yet is not a mismatch — there is
        // nothing to fail safe about.
        let f = filter(&["*.mjs"]);
        let items: Vec<String> = vec![];
        assert!(f.select(&items, |s: &String| s.clone()).is_empty());
    }

    #[test]
    fn an_unparseable_pattern_is_rejected() {
        let bad = vec!["[".to_string()];
        assert!(ConsumesFilter::compile(&bad).is_err());
        assert!(validate_pattern("[").is_err());
        assert!(validate_pattern("*.d.ts").is_ok());
    }
}
