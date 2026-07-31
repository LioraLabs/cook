use super::*;

#[test]
fn the_audit_case_renders_one_way() {
    // 61,500ms rendered four ways before COOK-392; this is the one way.
    assert_eq!(duration_ms(61_500), "1m01s");
}

#[test]
fn all_four_bands() {
    assert_eq!(duration_ms(0), "0ms");
    assert_eq!(duration_ms(999), "999ms");
    assert_eq!(duration_ms(1000), "1.0s");
    assert_eq!(duration_ms(1500), "1.5s");
    assert_eq!(duration_ms(59_949), "59.9s");
    assert_eq!(duration_ms(60_000), "1m00s");
    assert_eq!(duration_ms(3_599_000), "59m59s");
    assert_eq!(duration_ms(3_600_000), "1h00m00s");
    assert_eq!(duration_ms(7_509_000), "2h05m09s");
}

#[test]
fn hex_is_lowercase_and_padded() {
    assert_eq!(lower_hex(&[0x00, 0x0f, 0xff]), "000fff");
    assert_eq!(lower_hex(&[]), "");
}
