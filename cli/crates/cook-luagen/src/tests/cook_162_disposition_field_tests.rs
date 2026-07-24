use super::*;

#[test]
fn disposition_field_emits_local_and_pinned() {
    // I3: sharing is a plain string field (no reserved-keyword hack).
    let mut d = Disposition::default();
    d.sharing = cook_contracts::Sharing::Local;
    assert_eq!(disposition_field(&d), ", sharing = \"local\"");
    let mut d2 = Disposition::default();
    d2.sharing = cook_contracts::Sharing::Pinned;
    assert_eq!(disposition_field(&d2), ", sharing = \"pinned\"");
    let mut d3 = Disposition::default();
    d3.seal.insert("host".to_string());
    assert!(disposition_field(&d3).contains("seal = "));
    assert_eq!(disposition_field(&Disposition::default()), "");
}
