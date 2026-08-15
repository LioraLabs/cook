use super::CapCounter;

#[test]
fn accumulates_under_the_cap() {
    let mut c = CapCounter::new(10);
    assert!(c.add(4).is_ok());
    assert!(c.add(6).is_ok());
    assert_eq!(c.total(), 10);
    assert!(!c.exceeded(), "exactly at the cap is not over it");
}

#[test]
fn reports_once_the_total_crosses() {
    let mut c = CapCounter::new(10);
    assert!(c.add(11).is_err());
    assert!(c.exceeded());
    assert_eq!(c.message(), "artifact exceeds max_artifact_bytes (11); cap 10");
}

/// COOK-417: the message was written out three times (backend.rs:349,
/// cloud_backend.rs:307, cloud_backend.rs:524). The cloud path re-raises it
/// after ureq flattens the io::Error into a transport error, so `message`
/// must give the same text as `add`'s error at the same total.
#[test]
fn re_raising_gives_the_same_text_as_the_original_error() {
    let mut c = CapCounter::new(64);
    let from_add = c.add(100).unwrap_err();
    assert_eq!(from_add, c.message());
}

/// A reader reporting absurd counts must not wrap the total back under the
/// limit and turn a refusal into an accept.
#[test]
fn saturates_rather_than_wrapping() {
    let mut c = CapCounter::new(10);
    assert!(c.add(u64::MAX).is_err());
    assert!(c.add(u64::MAX).is_err());
    assert_eq!(c.total(), u64::MAX);
    assert!(c.exceeded());
}

#[test]
fn a_zero_cap_refuses_the_first_byte() {
    let mut c = CapCounter::new(0);
    assert!(c.add(1).is_err());
}
