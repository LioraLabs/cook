//! The size grammar stated once, as data.
//!
//! `cook-cache`'s `cloud_config_tests.rs` covers this parser through
//! `[cache] max_size`, and `cook-cli`'s `cache_gc_tests.rs` covers it through
//! `--max-size`. Both are consumer tests: they prove their own end reaches the
//! shared function. These are the law's own tests, and they exist so a change
//! to the grammar fails HERE, next to the definition, rather than in whichever
//! consumer's fixtures happened to name the affected form.

use super::{parse_size, SIZE_LITERAL_HELP};

#[test]
fn a_bare_number_is_bytes() {
    assert_eq!(parse_size("4096"), Some(4096));
    assert_eq!(parse_size("0"), Some(0));
}

#[test]
fn decimal_units_are_powers_of_1000() {
    assert_eq!(parse_size("1KB"), Some(1_000));
    assert_eq!(parse_size("1MB"), Some(1_000_000));
    assert_eq!(parse_size("20GB"), Some(20_000_000_000));
    assert_eq!(parse_size("1TB"), Some(1_000_000_000_000));
}

#[test]
fn binary_units_are_powers_of_1024() {
    assert_eq!(parse_size("1KiB"), Some(1024));
    assert_eq!(parse_size("1MiB"), Some(1024 * 1024));
    assert_eq!(parse_size("20GiB"), Some(21_474_836_480));
    assert_eq!(parse_size("1TiB"), Some(1024u64.pow(4)));
}

#[test]
fn a_unit_is_case_insensitive_and_may_be_spaced_off_the_number() {
    assert_eq!(parse_size("512mb"), Some(512_000_000));
    assert_eq!(parse_size("512 MB"), Some(512_000_000));
    assert_eq!(parse_size("512 mIb"), Some(512 * 1024 * 1024));
}

#[test]
fn surrounding_whitespace_is_not_part_of_the_literal() {
    assert_eq!(parse_size("  20GB  "), Some(20_000_000_000));
}

#[test]
fn a_fraction_truncates_toward_zero() {
    assert_eq!(parse_size("1.5GB"), Some(1_500_000_000));
    // 0.5 bytes is zero bytes, not one.
    assert_eq!(parse_size("0.5"), Some(0));
}

#[test]
fn a_sign_is_refused_including_the_negative_zero_that_a_comparison_would_admit() {
    // `-0.0 < 0.0` is false, so a range check would let these through; the
    // grammar refuses the sign itself, which is why it cannot.
    assert_eq!(parse_size("-5GB"), None);
    assert_eq!(parse_size("-0"), None);
    assert_eq!(parse_size("-0GB"), None);
    assert_eq!(parse_size("+5GB"), None);
}

#[test]
fn a_value_too_large_for_u64_is_refused_rather_than_saturated() {
    // The failure this refuses is silent: saturating would read as
    // "effectively no budget", which is the opposite of what a typo meant.
    assert_eq!(parse_size("999999999TB"), None);
    assert_eq!(parse_size("18446744073709551616"), None);
}

#[test]
fn a_literal_outside_the_grammar_is_refused() {
    assert_eq!(parse_size(""), None);
    assert_eq!(parse_size("twenty gigs"), None);
    assert_eq!(parse_size("GB"), None);
    assert_eq!(parse_size("20PB"), None);
    assert_eq!(parse_size("20GB extra"), None);
}

#[test]
fn every_unit_the_help_text_advertises_actually_parses() {
    // The constant and the parser are the two ends this module exists to
    // keep together: a unit added to one and not the other is exactly the
    // drift the shared literal was pulled out of cook-cache to prevent.
    for unit in ["B", "KB", "MB", "GB", "TB", "KiB", "MiB", "GiB", "TiB"] {
        assert!(
            SIZE_LITERAL_HELP.contains(unit),
            "{unit} parses but the help text does not advertise it"
        );
        assert!(
            parse_size(&format!("1{unit}")).is_some(),
            "the help text advertises {unit} but the parser refuses it"
        );
    }
}
