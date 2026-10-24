fn pair(byte: u8) -> [u8; 2] {
    const NIBBLE_BITS: u32 = 4;
    const NIBBLE_MASK: u8 = 0x0f;
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    [
        DIGITS[usize::from(byte >> NIBBLE_BITS)],
        DIGITS[usize::from(byte & NIBBLE_MASK)],
    ]
}

pub(crate) fn encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|&byte| pair(byte))
        .map(char::from)
        .collect()
}

#[cfg(kani)]
mod proofs {
    #[kani::proof]
    fn hexadecimal_identity_preserves_every_byte() {
        let byte: u8 = kani::any();
        let encoded = super::pair(byte);
        let decode = |digit: u8| {
            assert!(digit.is_ascii_digit() || (b'a'..=b'f').contains(&digit));
            if digit <= b'9' {
                digit - b'0'
            } else {
                digit - b'a' + 10
            }
        };
        assert_eq!((decode(encoded[0]) << 4) | decode(encoded[1]), byte);
        kani::cover!(byte == 0);
        kani::cover!(byte == u8::MAX);
    }

    #[kani::proof]
    #[kani::unwind(65)]
    fn hexadecimal_encoding_preserves_the_complete_digest() {
        let bytes: [u8; 32] = kani::any();
        let index = usize::from(kani::any::<u8>() % 32);
        let encoded = super::encode(&bytes);
        assert_eq!(encoded.len(), 64);
        assert_eq!(encoded.as_bytes()[index * 2], super::pair(bytes[index])[0]);
        assert_eq!(
            encoded.as_bytes()[index * 2 + 1],
            super::pair(bytes[index])[1]
        );
        kani::cover!(bytes == [0; 32]);
        kani::cover!(bytes == [u8::MAX; 32]);
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    #[test]
    fn identities_keep_the_sha256_known_answers() {
        assert_eq!(super::encode(&[0, 15, 16, 128, 255]), "000f1080ff");
        for (input, expected) in [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
        ] {
            assert_eq!(super::encode(&Sha256::digest(input)), expected);
        }
    }

    #[test]
    fn encoding_matches_standard_hexadecimal_for_every_byte_and_the_empty_input() {
        assert!(super::encode(&[]).is_empty());
        for byte in u8::MIN..=u8::MAX {
            assert_eq!(super::encode(&[byte]), format!("{byte:02x}"));
        }
        let bytes: Vec<_> = (u8::MIN..=u8::MAX).collect();
        let expected: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(super::encode(&bytes), expected);
    }
}
