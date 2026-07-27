use super::CapturedStream;

const CAP: usize = 64 * 1024;
const HEAD: usize = 16 * 1024;

#[test]
fn empty_stream_is_empty() {
    let stream = CapturedStream::from_bytes(b"");
    assert!(stream.is_empty());
    assert_eq!(stream.as_str(), "");
}

#[test]
fn input_through_the_cap_is_preserved_lossily() {
    let bytes = vec![b'x'; CAP];
    assert_eq!(
        CapturedStream::from_bytes(&bytes).as_str().as_bytes(),
        bytes
    );
    assert_eq!(CapturedStream::from_bytes(b"a\xffb").as_str(), "a\u{fffd}b");
}

#[test]
fn over_cap_input_retains_head_and_tail_with_exact_elision_count() {
    let bytes = vec![b'x'; CAP + 100];
    let stream = CapturedStream::from_bytes(&bytes);
    let marker = "\n... (100 bytes elided; showing the first 16384 and last 49152 bytes) ...\n";

    assert!(stream.as_str().starts_with(&"x".repeat(HEAD)));
    assert!(stream.as_str().contains(marker));
    assert!(stream.as_str().ends_with(&"x".repeat(CAP - HEAD)));
}

#[test]
fn viable_line_boundaries_are_preferred_and_accounted_in_source_bytes() {
    let mut bytes = vec![b'h'; HEAD - 4];
    bytes.extend_from_slice(b"\ncut");
    bytes.extend(vec![b'm'; 200]);
    bytes.extend_from_slice(b"discard\n");
    bytes.extend(vec![b't'; CAP - HEAD]);

    let stream = CapturedStream::from_bytes(&bytes);
    let marker = format!(
        "... ({} bytes elided; showing the first {} and last {} bytes) ...\n",
        211,
        HEAD - 3,
        CAP - HEAD
    );
    assert!(stream.as_str().starts_with(&"h".repeat(HEAD - 4)));
    assert!(
        stream.as_str().contains(&marker),
        "missing marker {marker:?} in {:?}",
        &stream.as_str()[HEAD.saturating_sub(32)..stream.as_str().len().min(HEAD + 128)]
    );
    assert!(stream.as_str().ends_with(&"t".repeat(CAP - HEAD)));
}

#[test]
fn lossy_decoding_handles_invalid_utf8_at_both_cut_boundaries() {
    let mut bytes = vec![b'a'; CAP + 2];
    bytes[HEAD - 1] = 0xf0;
    let tail_start = bytes.len() - (CAP - HEAD);
    bytes[tail_start] = 0x80;

    let rendered = CapturedStream::from_bytes(&bytes).as_str().to_owned();
    assert!(rendered.starts_with(&"a".repeat(HEAD - 1)));
    assert!(rendered.contains('\u{fffd}'));
    assert!(rendered.ends_with(&"a".repeat(CAP - HEAD - 1)));
    assert!(rendered.contains("2 bytes elided"));
}

#[test]
fn multibyte_utf8_crossing_both_cuts_is_decoded_lossily_without_panicking() {
    let mut bytes = vec![b'a'; CAP + 4];
    bytes[HEAD - 1..HEAD + 3].copy_from_slice("🦀".as_bytes());
    let tail_start = bytes.len() - (CAP - HEAD);
    bytes[tail_start - 2..tail_start + 2].copy_from_slice("🦀".as_bytes());

    let rendered = CapturedStream::from_bytes(&bytes).as_str().to_owned();
    assert!(rendered.contains('\u{fffd}'));
    assert!(rendered.contains("4 bytes elided"));
}
