pub(super) fn append_path(path: &[u8], output: &mut Vec<u8>) -> bool {
    if path.is_empty() || path.contains(&0) {
        return false;
    }
    output.extend_from_slice(path);
    output.push(0);
    true
}

#[cfg(kani)]
mod proofs {
    use super::append_path;

    #[kani::proof]
    #[kani::unwind(34)]
    fn index_input_preserves_paths_and_rejects_embedded_delimiters() {
        let bytes: [u8; 32] = kani::any();
        let length: u8 = kani::any();
        kani::assume(length <= 32);
        let path = &bytes[..usize::from(length)];
        let mut encoded = Vec::new();
        let accepted = append_path(path, &mut encoded);
        assert_eq!(accepted, !path.is_empty() && !path.contains(&0));
        if accepted {
            assert_eq!(encoded.len(), path.len() + 1);
            assert_eq!(&encoded[..path.len()], path);
            assert_eq!(encoded[path.len()], 0);
        } else {
            assert!(encoded.is_empty());
        }
        kani::cover!(accepted && path[0] == b'-');
        kani::cover!(accepted && path.contains(&b'\n'));
        kani::cover!(!accepted && !path.is_empty());
    }
}
