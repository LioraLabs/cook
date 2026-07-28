use super::*;

fn out(text: &str) -> OutputChunk {
    OutputChunk::new(OutputStream::Stdout, text).unwrap()
}
fn err(text: &str) -> OutputChunk {
    OutputChunk::new(OutputStream::Stderr, text).unwrap()
}

#[test]
fn the_scalar_half_carries_what_a_reader_needs_without_a_fetch() {
    let o = Observation::new(1_500, 1_753_600_000, Some("input changed: a.c".into()), 4_200);
    assert_eq!(o.duration_ms(), 1_500);
    assert_eq!(o.recorded_at(), 1_753_600_000);
    assert_eq!(o.cause(), Some("input changed: a.c"));
    // The point of `log_bytes`: report the weight without reading the log.
    assert_eq!(o.log_bytes(), 4_200);
}

#[test]
fn a_silent_unit_has_an_observation_with_no_log() {
    // Distinguishable from "nothing recorded" only by the observation existing
    // at all, which is what makes the record's existence the verdict.
    let o = Observation::new(12, 1_753_600_000, None, 0);
    assert_eq!(o.log_bytes(), 0);
    assert_eq!(o.cause(), None);
}

#[test]
fn a_log_round_trips_with_its_order_and_streams_intact() {
    let log = OutputLog::new(
        vec![
            out("configure: checking cc"),
            err("configure: no pkg-config"),
            out("make: cc -c a.c"),
            err("a.c:12: unused var"),
        ],
        0,
    );
    let decoded = OutputLog::decode(&log.encode()).expect("round trip");
    assert_eq!(decoded, log);
    // The ordering guarantee, at the level this type is responsible for: the
    // stderr chunk of call one precedes the stdout chunk of call two.
    let streams: Vec<OutputStream> = decoded.chunks().iter().map(|c| c.stream()).collect();
    assert_eq!(
        streams,
        vec![
            OutputStream::Stdout,
            OutputStream::Stderr,
            OutputStream::Stdout,
            OutputStream::Stderr
        ]
    );
}

#[test]
fn an_empty_log_round_trips() {
    let log = OutputLog::default();
    assert_eq!(OutputLog::decode(&log.encode()).unwrap(), log);
    assert!(!log.is_truncated());
}

#[test]
fn invalid_utf8_survives_the_encoding() {
    // The reason this is a binary format and not JSON. A compiler emitting a
    // stray byte must round-trip; a lossy conversion here would corrupt the
    // capture permanently, and it is stored content-addressed.
    let raw = OutputChunk::new(OutputStream::Stderr, vec![b'o', b'k', 0xff, b'!']).unwrap();
    let log = OutputLog::new(vec![raw], 0);
    let decoded = OutputLog::decode(&log.encode()).unwrap();
    assert_eq!(decoded.chunks()[0].bytes(), b"ok\xff!");
}

#[test]
fn truncation_is_recorded_and_survives_the_round_trip() {
    let log = OutputLog::new(vec![out("kept")], 9_000);
    assert!(log.is_truncated());
    let decoded = OutputLog::decode(&log.encode()).unwrap();
    assert_eq!(decoded.truncated_bytes(), 9_000);
    assert!(decoded.is_truncated());
}

#[test]
fn truncate_keeps_the_head_and_the_tail() {
    // Head and tail are what a reader needs: a compiler's first errors and its
    // summary. The middle of a runaway log is the part nobody scrolls to.
    let log = OutputLog::new(
        vec![out("AAAA"), out("BBBB"), out("CCCC"), out("DDDD")],
        0,
    );
    let t = log.truncate_to(8);
    let kept: Vec<String> = t.chunks().iter().map(|c| c.lossy().into_owned()).collect();
    assert_eq!(kept, vec!["AAAA", "DDDD"]);
    assert_eq!(t.truncated_bytes(), 8, "the two dropped chunks are reported");
}

#[test]
fn truncate_is_a_no_op_under_the_cap() {
    let log = OutputLog::new(vec![out("AAAA"), out("BBBB")], 0);
    let t = log.clone().truncate_to(1024);
    assert_eq!(t, log);
    assert!(!t.is_truncated());
}

#[test]
fn truncate_to_zero_keeps_nothing_and_says_so() {
    let log = OutputLog::new(vec![out("AAAA"), out("BBBB")], 0);
    let t = log.truncate_to(0);
    assert!(t.chunks().is_empty());
    assert_eq!(t.truncated_bytes(), 8);
}

#[test]
fn a_single_oversized_chunk_still_keeps_its_head_and_tail() {
    // The case that matters most, and the one chunk-granular truncation got
    // wrong: a chunk is one spawn's bytes on one stream, so a chatty `cc`
    // invocation is ONE chunk. Dropping it whole for overrunning the cap kept
    // neither the first errors nor the summary line.
    let log = OutputLog::new(vec![out("AAAABBBBCCCCDDDD")], 0);
    let t = log.truncate_to(8);
    let kept: String = t.chunks().iter().map(|c| c.lossy().into_owned()).collect();
    assert_eq!(kept, "AAAADDDD", "head and tail of the one chunk survive");
    assert_eq!(t.truncated_bytes(), 8, "the elided middle is reported");
}

#[test]
fn a_split_chunk_keeps_its_stream_tag() {
    // A split retains a prefix or suffix of what one spawn wrote to one
    // stream; it must not relabel which stream that was.
    let log = OutputLog::new(vec![err("AAAABBBBCCCC")], 0);
    let t = log.truncate_to(4);
    assert!(t.chunks().iter().all(|c| c.stream() == OutputStream::Stderr));
}

#[test]
fn truncate_never_keeps_more_than_the_cap_or_overlaps() {
    // The head is a prefix of the concatenated stream and the tail a suffix;
    // they must never meet in the middle and duplicate bytes.
    for cap in 0u64..24 {
        let log = OutputLog::new(vec![out("AAAAAAA"), err("BBB"), out("CCCCC")], 0);
        let total = 15u64;
        let t = log.truncate_to(cap);
        let kept: u64 = t.chunks().iter().map(|c| c.bytes().len() as u64).sum();
        assert!(kept <= cap.min(total), "cap {cap} kept {kept}");
        assert_eq!(
            t.truncated_bytes(),
            total - kept,
            "cap {cap}: elision must account for every dropped byte"
        );
        // What survives must be a prefix of the original followed by a suffix
        // of it: the drop came from the middle and nowhere else.
        let original = "AAAAAAABBBCCCCC";
        let text: String = t.chunks().iter().map(|c| c.lossy().into_owned()).collect();
        assert!(
            (0..=text.len()).any(|k| original.starts_with(&text[..k])
                && original.ends_with(&text[k..])),
            "cap {cap} produced {text:?}, which is not a head+tail of the original"
        );
    }
}

#[test]
fn a_truncated_log_round_trips_with_its_split_chunks() {
    let log = OutputLog::new(vec![out("AAAABBBBCCCC"), err("DDDD")], 0).truncate_to(8);
    let decoded = OutputLog::decode(&log.encode()).expect("round trip");
    assert_eq!(decoded, log);
    assert!(decoded.is_truncated());
}

#[test]
fn a_foreign_blob_is_refused_at_the_door() {
    // A caller reading the content-addressed store treats Err as an absent
    // observation. It must not decode into a plausible-looking empty log.
    assert!(OutputLog::decode(b"").is_err());
    assert!(OutputLog::decode(b"not a cook log at all").is_err());
    assert!(OutputLog::decode(b"COOKLOG\0").is_err(), "magic alone is not a log");
}

#[test]
fn a_future_version_is_refused_rather_than_misread() {
    let mut bytes = OutputLog::new(vec![out("x")], 0).encode();
    bytes[8] = 99; // version
    let e = OutputLog::decode(&bytes).unwrap_err();
    assert!(e.contains("unsupported version"), "got: {e}");
}

#[test]
fn a_blob_cut_mid_field_is_refused() {
    let full = OutputLog::new(vec![out("hello"), err("world")], 3).encode();

    // Cutting inside any field is refused: the decoder checks it has the bytes
    // before reading them, at every field, so no prefix decodes into a
    // plausible-looking log with a garbage chunk on the end.
    let header = MAGIC.len() + 4 + 8;
    let boundaries: Vec<usize> = {
        // The positions a chunk ends at: header, then after each 1+4+len block.
        let mut b = vec![header];
        let mut p = header;
        for len in [5usize, 5] {
            p += 1 + 4 + len;
            b.push(p);
        }
        b
    };
    for cut in 1..full.len() {
        if boundaries.contains(&cut) {
            continue;
        }
        assert!(
            OutputLog::decode(&full[..cut]).is_err(),
            "a blob cut mid-field at {cut} decoded"
        );
    }
    assert!(OutputLog::decode(&full).is_ok());
}

#[test]
fn a_blob_cut_exactly_at_a_chunk_boundary_decodes_as_a_shorter_log() {
    // Stated because it is a real property of this encoding rather than an
    // oversight: the format is not self-delimiting, so a blob truncated exactly
    // between chunks is indistinguishable from a log that had fewer chunks.
    //
    // That is safe HERE and only here, because nothing hands this decoder bytes
    // it has not already integrity-checked: an observation is read from the
    // content-addressed store, whose reader verifies the content hash on
    // restore (§{exec.cache.integrity}, CS-0054). Detecting truncation is the
    // store's job, and duplicating it with a length prefix or a checksum would
    // be a second answer to a question already answered.
    //
    // If this encoding ever reaches bytes that arrive unverified, this test is
    // the one that has to change, and it should fail loudly when it does.
    let full = OutputLog::new(vec![out("hello"), err("world")], 3).encode();
    let after_first_chunk = MAGIC.len() + 4 + 8 + 1 + 4 + 5;
    let short = OutputLog::decode(&full[..after_first_chunk]).expect("decodes");
    assert_eq!(short.chunks().len(), 1);
    assert_eq!(short.chunks()[0].lossy(), "hello");
    // The header-only prefix is likewise a valid empty log.
    let header_only = OutputLog::decode(&full[..MAGIC.len() + 4 + 8]).expect("decodes");
    assert!(header_only.chunks().is_empty());
    assert_eq!(header_only.truncated_bytes(), 3);
}

#[test]
fn an_unknown_stream_tag_is_refused() {
    let mut bytes = OutputLog::new(vec![out("x")], 0).encode();
    let tag = 8 + 4 + 8; // magic + version + truncated_bytes
    bytes[tag] = 7;
    assert!(OutputLog::decode(&bytes).unwrap_err().contains("unknown stream tag"));
}
