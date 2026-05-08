//! Argument parsing for `cook pull`. `PullArgs` is parsed from `argv[1..]`
//! (i.e. without the `pull` token); use [`parse`].

use super::errors::PullError;

#[derive(clap::Args, Debug, Clone)]
#[command(about = "Pull cook_modules from a configured HTTP(S) registry.")]
pub struct PullArgs {
    /// Module names to pull (empty when --all or --list is used).
    #[arg(value_name = "NAME")]
    pub names: Vec<String>,

    /// Pull every module the registry exposes.
    #[arg(long)]
    pub all: bool,

    /// Print available module names without writing anything.
    #[arg(long)]
    pub list: bool,

    /// Treat all overwrite prompts as "yes". Does NOT bypass the trust prompt.
    #[arg(long)]
    pub force: bool,

    /// Non-interactive consent for the trust-on-first-use prompt.
    #[arg(long = "accept-trust")]
    pub accept_trust: bool,

    /// Error on any prompt instead of asking. Implied when stdin is not a TTY.
    #[arg(long = "non-interactive")]
    pub non_interactive: bool,

    /// One-shot registry URL override (highest precedence).
    #[arg(long = "registry", value_name = "URL")]
    pub registry: Option<String>,
}

/// Parse argv into `PullArgs` with cross-arg validation.
///
/// `argv` is the slice **after** the `pull` token. `--help` and `--version`
/// are handled by clap (printed and process exited cleanly); real parse
/// errors and our cross-arg rules are surfaced as `PullError::BadArgs`.
pub fn parse(argv: &[String]) -> Result<PullArgs, PullError> {
    // clap derive expects the binary name as argv[0], so prepend a synthetic one.
    let mut full: Vec<String> = vec!["cook pull".to_string()];
    full.extend_from_slice(argv);

    // Wrap PullArgs in a top-level Parser so try_parse_from still works for
    // the integration-test helper. The wrapper is private to this function.
    #[derive(clap::Parser)]
    #[command(name = "cook pull")]
    struct PullParser {
        #[command(flatten)]
        inner: PullArgs,
    }
    use clap::Parser as _;
    let args = match PullParser::try_parse_from(full).map(|p| p.inner) {
        Ok(a) => a,
        Err(e) => {
            // For --help and --version, let clap's own exit-cleanly path handle it.
            // For real parse errors, print clap's pretty message and surface BadArgs.
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                e.exit(); // never returns; prints to stdout, exits 0
            }
            e.print().ok();
            return Err(PullError::BadArgs {
                reason: String::new(),
            });
        }
    };

    validate(&args)?;
    Ok(args)
}

fn validate(args: &PullArgs) -> Result<(), PullError> {
    if args.list && args.all {
        return Err(PullError::BadArgs {
            reason: "--list and --all are mutually exclusive".into(),
        });
    }
    if args.list && !args.names.is_empty() {
        return Err(PullError::BadArgs {
            reason: "--list does not take module names".into(),
        });
    }
    if args.all && !args.names.is_empty() {
        return Err(PullError::BadArgs {
            reason: "--all does not take module names".into(),
        });
    }
    if !args.list && !args.all && args.names.is_empty() {
        return Err(PullError::BadArgs {
            reason: "no modules specified; pass NAME, --all, or --list".into(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_only() {
        let a = parse(&args(&["cpp"])).unwrap();
        assert_eq!(a.names, vec!["cpp"]);
        assert!(!a.all && !a.list && !a.force);
    }

    #[test]
    fn multiple_names() {
        let a = parse(&args(&["cpp", "rust"])).unwrap();
        assert_eq!(a.names, vec!["cpp", "rust"]);
    }

    #[test]
    fn list_alone() {
        let a = parse(&args(&["--list"])).unwrap();
        assert!(a.list);
    }

    #[test]
    fn all_alone() {
        let a = parse(&args(&["--all"])).unwrap();
        assert!(a.all);
    }

    #[test]
    fn list_with_name_rejected() {
        let e = parse(&args(&["--list", "cpp"])).unwrap_err();
        assert!(matches!(e, PullError::BadArgs { .. }));
    }

    #[test]
    fn all_with_name_rejected() {
        let e = parse(&args(&["--all", "cpp"])).unwrap_err();
        assert!(matches!(e, PullError::BadArgs { .. }));
    }

    #[test]
    fn list_and_all_rejected() {
        let e = parse(&args(&["--list", "--all"])).unwrap_err();
        assert!(matches!(e, PullError::BadArgs { .. }));
    }

    #[test]
    fn empty_rejected() {
        let e = parse(&args(&[])).unwrap_err();
        assert!(matches!(e, PullError::BadArgs { .. }));
    }

    #[test]
    fn force_and_accept_trust_independent() {
        let a = parse(&args(&["--force", "--accept-trust", "cpp"])).unwrap();
        assert!(a.force && a.accept_trust);
    }

    #[test]
    fn non_interactive_flag_parsed() {
        let a = parse(&args(&["--non-interactive", "cpp"])).unwrap();
        assert!(a.non_interactive);
        assert_eq!(a.names, vec!["cpp"]);
    }

    #[test]
    fn registry_override_recorded() {
        let a = parse(&args(&["--registry", "https://example.test/r", "cpp"])).unwrap();
        assert_eq!(a.registry.as_deref(), Some("https://example.test/r"));
    }
}
