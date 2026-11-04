use untrusted::{EndOfInput, Input, Reader};

#[test]
fn bounded_inputs_match_slice_cursor_reference() {
    let mut cases = 0;
    for len in 0..=6 {
        for bits in 0..(1usize << len) {
            let bytes: Vec<u8> = (0..len).map(|i| ((bits >> i) & 1) as u8).collect();
            for start in 0..=len {
                for count in (0..=len + 1).chain(std::iter::once(usize::MAX)) {
                    let mut reader = Reader::new(Input::from(&bytes));
                    reader.skip(start).unwrap();
                    assert_eq!(reader.at_end(), start == len);
                    assert_eq!(reader.peek(0), bytes.get(start) == Some(&0));
                    assert_eq!(reader.peek(1), bytes.get(start) == Some(&1));
                    let expected_end = start.checked_add(count).filter(|end| *end <= len);
                    match (reader.read_bytes(count), expected_end) {
                        (Ok(actual), Some(end)) => {
                            assert_eq!(actual.as_slice_less_safe(), &bytes[start..end]);
                            assert_eq!(reader.read_bytes_to_end().as_slice_less_safe(), &bytes[end..]);
                        }
                        (Err(EndOfInput), None) => {
                            // Failed reads do not advance the reader.
                            assert_eq!(reader.read_bytes_to_end().as_slice_less_safe(), &bytes[start..]);
                        }
                        _ => panic!("reference mismatch: {len}/{start}/{count}"),
                    }
                    cases += 1;
                }
                let mut reader = Reader::new(Input::from(&bytes));
                reader.skip(start).unwrap();
                match (reader.read_byte(), bytes.get(start)) {
                    (Ok(a), Some(b)) => assert_eq!(a, *b),
                    (Err(EndOfInput), None) => (),
                    _ => panic!("read_byte mismatch"),
                }
            }
        }
    }
    assert!(cases > 5_000);
}

#[test]
fn full_and_partial_reads_enforce_consumption_without_cursor_replacement() {
    let bytes = b"abcdef";
    for count in 0..=bytes.len() {
        let input = Input::from(bytes);
        let result = input.read_all("remaining", |r| {
            r.skip(count).unwrap();
            Ok::<_, &str>(())
        });
        assert_eq!(result, if count == bytes.len() { Ok(()) } else { Err("remaining") });
        for start in 0..=bytes.len() - count {
            let mut reader = Reader::new(input);
            reader.skip(start).unwrap();
            let (consumed, result) = reader.read_partial(|r| {
                r.skip(count).unwrap();
                Ok::<_, ()>(count)
            }).unwrap();
            assert_eq!(consumed.as_slice_less_safe(), &bytes[start..start+count]);
            assert_eq!(result, count);
        }
    }
    assert_eq!(untrusted::read_all_optional(None, (), |r| {
        assert!(r.is_none()); Ok::<_, ()>(5)
    }), Ok(5));
    assert_eq!(untrusted::read_all_optional(Some(Input::from(b"x")), (), |r| {
        r.unwrap().skip_to_end(); Ok::<_, ()>(6)
    }), Ok(6));
}
