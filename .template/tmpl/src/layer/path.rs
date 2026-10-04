#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Error {
    Empty,
    Absolute,
    Traversal,
}

pub(super) fn validate(bytes: &[u8]) -> Result<(), Error> {
    if bytes.is_empty() {
        return Err(Error::Empty);
    }
    if rooted(bytes) {
        return Err(Error::Absolute);
    }
    let mut segment = Segment::Empty;
    for &byte in bytes {
        if matches!(byte, b'/' | b'\\') {
            if segment == Segment::Parent {
                return Err(Error::Traversal);
            }
            segment = Segment::Empty;
        } else {
            segment = segment.observe(byte);
        }
    }
    if segment == Segment::Parent {
        return Err(Error::Traversal);
    }
    Ok(())
}

const fn rooted(bytes: &[u8]) -> bool {
    matches!(bytes, [b'/' | b'\\', ..] | [b'a'..=b'z' | b'A'..=b'Z', b':', ..])
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Segment {
    Empty,
    Dot,
    Parent,
    Other,
}

impl Segment {
    const fn observe(self, byte: u8) -> Self {
        match (self, byte) {
            (Self::Empty, b'.') => Self::Dot,
            (Self::Dot, b'.') => Self::Parent,
            _ => Self::Other,
        }
    }
}

#[cfg(kani)]
mod proofs {
    use super::{Error, rooted, validate};

    #[kani::proof]
    #[kani::unwind(33)]
    fn portable_roots_cannot_be_admitted_as_relative() {
        let bytes: [u8; 32] = kani::any();
        let length: u8 = kani::any();
        kani::assume(length <= 32);
        let input = &bytes[..usize::from(length)];
        assert_eq!(
            rooted(input),
            length >= 1 && matches!(bytes[0], b'/' | b'\\')
                || length >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
        );
        let result = validate(input);
        let mut parent = false;
        for index in 0..input.len() {
            parent |= input[index] == b'.'
                && input.get(index + 1) == Some(&b'.')
                && (index == 0 || matches!(input[index - 1], b'/' | b'\\'))
                && (index + 2 == input.len()
                    || input
                        .get(index + 2)
                        .is_some_and(|byte| matches!(byte, b'/' | b'\\')));
        }
        let expected = if input.is_empty() {
            Err(Error::Empty)
        } else if rooted(input) {
            Err(Error::Absolute)
        } else if parent {
            Err(Error::Traversal)
        } else {
            Ok(())
        };
        assert_eq!(result, expected);
        if result.is_ok() {
            assert!(!input.is_empty());
            assert!(!rooted(input));
            assert!(!parent);
        }
        if input.is_empty() {
            assert_eq!(result, Err(Error::Empty));
        } else if rooted(input) {
            assert_eq!(result, Err(Error::Absolute));
        }
        assert_eq!(validate(b"safe/../escape"), Err(Error::Traversal));
        assert_eq!(validate(b"safe\\..\\escape"), Err(Error::Traversal));
        assert!(validate(b"1:relative").is_ok());
        kani::cover!(result.is_ok());
        kani::cover!(result == Err(Error::Absolute));
        kani::cover!(result == Err(Error::Traversal));
    }

    #[kani::proof]
    fn reject_rooted_path_probe() {
        assert!(validate(b"/etc/passwd").is_ok());
    }
}
