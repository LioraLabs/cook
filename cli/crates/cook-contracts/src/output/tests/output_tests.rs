use super::*;

#[test]
fn chunk_carries_its_stream_and_bytes() {
    let c = OutputChunk::new(OutputStream::Stderr, b"warning: unused".to_vec()).unwrap();
    assert_eq!(c.stream(), OutputStream::Stderr);
    assert_eq!(c.bytes(), b"warning: unused");
}

#[test]
fn empty_bytes_produce_no_chunk() {
    // A spawn that wrote nothing to a stream contributes no chunk for it, so
    // "did this unit print anything" stays a question about emptiness of the
    // sequence rather than about the lengths of its members.
    assert!(OutputChunk::new(OutputStream::Stdout, Vec::new()).is_none());
    assert!(OutputChunk::new(OutputStream::Stderr, "").is_none());
}

#[test]
fn invalid_utf8_survives_capture_and_is_replaced_only_at_render() {
    // The motivating case: a compiler emits a stray byte inside otherwise
    // ordinary diagnostics. Capturing as String would destroy it here; the
    // bytes must round-trip and the replacement must happen at `lossy()`.
    let raw = vec![b'o', b'k', 0xff, b'!'];
    let c = OutputChunk::new(OutputStream::Stdout, raw.clone()).unwrap();
    assert_eq!(c.bytes(), &raw[..]);
    assert_eq!(c.lossy(), "ok\u{fffd}!");
}

#[test]
fn a_sequence_preserves_the_order_of_the_spawns_that_made_it() {
    // CS-0188's ordering guarantee, at the level this type is responsible for:
    // the sequence is ordered, and two chunks from different spawns keep their
    // relative order regardless of which stream each came from.
    let seq: Vec<OutputChunk> = vec![
        OutputChunk::new(OutputStream::Stdout, "configure: checking cc").unwrap(),
        OutputChunk::new(OutputStream::Stderr, "configure: no pkg-config").unwrap(),
        OutputChunk::new(OutputStream::Stdout, "make: cc -c a.c").unwrap(),
        OutputChunk::new(OutputStream::Stderr, "a.c:12: unused var").unwrap(),
    ];
    let rendered: Vec<String> = seq.iter().map(|c| c.lossy().into_owned()).collect();
    assert_eq!(
        rendered,
        vec![
            "configure: checking cc",
            "configure: no pkg-config",
            "make: cc -c a.c",
            "a.c:12: unused var",
        ]
    );
    // Filtering to one stream keeps that stream's own order.
    let errs: Vec<String> = seq
        .iter()
        .filter(|c| c.stream() == OutputStream::Stderr)
        .map(|c| c.lossy().into_owned())
        .collect();
    assert_eq!(errs, vec!["configure: no pkg-config", "a.c:12: unused var"]);
}
