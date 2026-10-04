#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Error {
    Length,
    Digit(u8),
}

pub(super) fn decode(bytes: &[u8]) -> Result<[u8; 32], Error> {
    if bytes.len() != 64 {
        return Err(Error::Length);
    }
    let mut out = [0; 32];
    for (index, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let high = nibble(pair[0]).ok_or(Error::Digit(pair[0]))?;
        let low = nibble(pair[1]).ok_or(Error::Digit(pair[1]))?;
        out[index] = (high << 4) | low;
    }
    Ok(out)
}

const fn nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(kani)]
mod proofs {
    use super::{Error, decode, nibble};

    #[kani::proof]
    fn nibble_accepts_exactly_ascii_hexadecimal_digits() {
        let byte: u8 = kani::any();
        let value = nibble(byte);
        assert_eq!(value.is_some(), byte.is_ascii_hexdigit());
        if let Some(value) = value {
            assert!(value < 16);
            let lower = byte.to_ascii_lowercase();
            let expected = if lower.is_ascii_digit() {
                lower - b'0'
            } else {
                lower - b'a' + 10
            };
            assert_eq!(value, expected);
        }
        kani::cover!(value.is_some());
        kani::cover!(value.is_none());
    }

    #[kani::proof]
    #[kani::unwind(65)]
    fn decoder_preserves_every_byte_and_rejects_invalid_digits() {
        let input: [u8; 64] = kani::any();
        let result = decode(&input);
        let invalid = input.iter().copied().find(|byte| !byte.is_ascii_hexdigit());
        assert_eq!(result.is_ok(), invalid.is_none());
        if let Ok(output) = result {
            for (index, pair) in input.as_chunks::<2>().0.iter().enumerate() {
                assert_eq!(
                    Some(output[index]),
                    nibble(pair[0])
                        .zip(nibble(pair[1]))
                        .map(|(high, low)| high * 16 + low)
                );
            }
        }
        if let Some(expected) = invalid {
            assert_eq!(result, Err(Error::Digit(expected)));
        }
        assert_eq!(decode(&input[..63]), Err(Error::Length));
        kani::cover!(result.is_ok());
        kani::cover!(result.is_err());
    }

    #[kani::proof]
    fn reject_invalid_digit_probe() {
        assert!(decode(&[b'g'; 64]).is_ok());
    }
}
