//! Minimal read-only protobuf wire-format decoder.
//!
//! Antigravity stores each conversation's step metadata as protobuf blobs in
//! SQLite. Rather than pulling in a full protobuf runtime (and a `.proto` file
//! we don't own), this module decodes just the wire format the adapter needs:
//! varints, fixed32/64 and length-delimited fields. It is deliberately strict
//! about malformed data — a bad blob yields `None` so a single corrupt step is
//! skipped instead of failing the whole conversation.

/// The value carried by one decoded protobuf field.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WireValue<'a> {
    Varint(u64),
    Fixed64(u64),
    Len(&'a [u8]),
    Fixed32(u32),
}

/// One decoded protobuf field (field number + value).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field<'a> {
    pub number: u32,
    pub value: WireValue<'a>,
}

/// Decode a protobuf message into its top-level fields. Returns `None` on any
/// malformed input: a truncated varint, a length running past the buffer, an
/// unknown wire type (including deprecated groups), or a tag that overflows
/// the 64-bit varint budget.
pub(crate) fn parse_fields(buf: &[u8]) -> Option<Vec<Field<'_>>> {
    let mut fields = Vec::new();
    let mut i = 0usize;
    while i < buf.len() {
        let (tag, next) = read_varint(buf, i)?;
        i = next;
        if tag == 0 {
            return None;
        }
        let number = (tag >> 3) as u32;
        let wire = (tag & 7) as u8;
        let value = match wire {
            0 => {
                let (v, next) = read_varint(buf, i)?;
                i = next;
                WireValue::Varint(v)
            }
            1 => {
                let bytes = buf.get(i..i + 8)?;
                i += 8;
                WireValue::Fixed64(u64::from_le_bytes(bytes.try_into().ok()?))
            }
            2 => {
                let (len, next) = read_varint(buf, i)?;
                i = next;
                let len = len as usize;
                let slice = buf.get(i..i + len)?;
                i += len;
                WireValue::Len(slice)
            }
            5 => {
                let bytes = buf.get(i..i + 4)?;
                i += 4;
                WireValue::Fixed32(u32::from_le_bytes(bytes.try_into().ok()?))
            }
            // Groups (3/4) and anything else: we never expect them, and
            // skipping a group safely needs recursive tag parsing — bail.
            _ => return None,
        };
        fields.push(Field { number, value });
    }
    Some(fields)
}

/// Read a base-128 varint starting at `i`, returning `(value, next_index)`.
/// `None` when the buffer runs out or the varint exceeds 10 bytes (can't fit a
/// `u64`).
fn read_varint(buf: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    loop {
        let b = *buf.get(i)?;
        i += 1;
        result |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some((result, i));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

/// Value of the first field with the given number that carries a varint.
pub(crate) fn first_varint(fields: &[Field<'_>], number: u32) -> Option<u64> {
    fields
        .iter()
        .find(|f| f.number == number)
        .and_then(|f| match f.value {
            WireValue::Varint(v) => Some(v),
            _ => None,
        })
}

/// Payload of the first field with the given number that is length-delimited.
pub(crate) fn first_len<'a>(fields: &[Field<'a>], number: u32) -> Option<&'a [u8]> {
    fields
        .iter()
        .find(|f| f.number == number)
        .and_then(|f| match f.value {
            WireValue::Len(v) => Some(v),
            _ => None,
        })
}

/// The first `file:///...` string found in a length-delimited field, recursing
/// into nested messages. In `trajectory_metadata_blob.data` the workspace URI
/// sits two levels down (field 1 wraps a sub-message whose field 1 is the
/// URI); recursion makes the exact nesting irrelevant.
pub(crate) fn first_file_uri<'a>(fields: &[Field<'a>]) -> Option<&'a [u8]> {
    first_file_uri_depth(fields, 0)
}

fn first_file_uri_depth<'a>(fields: &[Field<'a>], depth: usize) -> Option<&'a [u8]> {
    if depth > 8 {
        return None;
    }
    for f in fields {
        if let WireValue::Len(v) = f.value {
            if v.starts_with(b"file:///") {
                return Some(v);
            }
            if let Some(inner) = parse_fields(v) {
                if let Some(uri) = first_file_uri_depth(&inner, depth + 1) {
                    return Some(uri);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_varint_len_and_nested() {
        let mut buf = vec![0x08, 0x96, 0x01]; // field 1, varint 150
        let uri = b"file:///C:/proj";
        buf.extend_from_slice(&[0x12, 0x0f]); // field 2, len 15
        buf.extend_from_slice(uri);
        let fields = parse_fields(&buf).unwrap();
        assert_eq!(first_varint(&fields, 1), Some(150));
        assert_eq!(first_len(&fields, 2), Some(uri.as_slice()));
        assert_eq!(first_file_uri(&fields), Some(uri.as_slice()));
    }

    #[test]
    fn finds_file_uri_two_levels_deep() {
        // field 1 wraps a sub-message whose field 1 is the URI.
        let mut inner = vec![0x0a, 17];
        inner.extend_from_slice(b"file:///a/b/c/dir");
        let mut outer = vec![0x0a, inner.len() as u8];
        outer.extend_from_slice(&inner);
        let fields = parse_fields(&outer).unwrap();
        assert_eq!(first_file_uri(&fields), Some(b"file:///a/b/c/dir".as_slice()));
    }

    #[test]
    fn rejects_truncated_and_unknown_wire_types() {
        // Length runs past the buffer.
        assert!(parse_fields(&[0x0a, 0x10, 0x01, 0x02]).is_none());
        // Unknown wire type 6.
        assert!(parse_fields(&[0x0e, 0x01]).is_none());
        // Zero tag.
        assert!(parse_fields(&[0x00]).is_none());
    }
}
