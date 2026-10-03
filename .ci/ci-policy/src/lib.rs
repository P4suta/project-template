#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conclusion {
    Success,
    Failure,
    Cancelled,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    Empty,
    Passed,
    Rejected,
}

impl GateState {
    pub fn observe(self, conclusion: Conclusion) -> Self {
        match (self, conclusion) {
            (Self::Empty | Self::Passed, Conclusion::Success) => Self::Passed,
            (Self::Rejected, _)
            | (_, Conclusion::Failure | Conclusion::Cancelled | Conclusion::Skipped) => {
                Self::Rejected
            }
        }
    }

    pub fn passed(self) -> bool {
        self == Self::Passed
    }
}

pub fn gate(conclusions: &[Conclusion]) -> bool {
    conclusions
        .iter()
        .copied()
        .fold(GateState::Empty, GateState::observe)
        .passed()
}

pub fn exact_hash(bytes: &[u8], length: usize) -> bool {
    bytes.len() == length && bytes.iter().all(u8::is_ascii_hexdigit)
}

fn safe_path(path: &str, minimum_segments: usize) -> bool {
    let segments: Vec<_> = path.split('/').collect();
    segments.len() >= minimum_segments
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && *segment != "."
                && *segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
}

pub fn immutable_reference(reference: &str) -> bool {
    if let Some(path) = reference.strip_prefix("$/") {
        return path.is_empty() || safe_path(path, 1);
    }
    if let Some(path) = reference.strip_prefix("./") {
        return path.is_empty() || safe_path(path, 1);
    }
    if let Some(image) = reference.strip_prefix("docker://") {
        return image.rsplit_once("@sha256:").is_some_and(|(path, digest)| {
            !path.is_empty() && !path.contains('@') && exact_hash(digest.as_bytes(), 64)
        });
    }
    reference
        .rsplit_once('@')
        .is_some_and(|(path, revision)| safe_path(path, 2) && exact_hash(revision.as_bytes(), 40))
}

pub mod hooks;
pub mod json;
pub mod local;
pub mod proof;
pub mod tools;
pub mod verification;
pub mod workflow;

pub const REVISION: &str = match option_env!("CI_POLICY_REVISION") {
    Some(revision) => revision,
    None => "development",
};

pub fn same_revision(expected: &[u8], actual: &[u8]) -> bool {
    exact_hash(expected, 40) && exact_hash(actual, 40) && expected == actual
}

#[cfg(kani)]
mod proofs {
    use super::{Conclusion, GateState, exact_hash, gate, same_revision};

    #[kani::proof]
    #[kani::unwind(41)]
    fn installed_policy_requires_the_reviewed_revision() {
        let expected: [u8; 40] = kani::any();
        let actual: [u8; 40] = kani::any();
        let accepted = same_revision(&expected, &actual);
        assert!(!accepted || (expected == actual && exact_hash(&expected, 40)));
        assert_eq!(
            same_revision(&expected, &expected),
            exact_hash(&expected, 40)
        );
        assert!(!same_revision(&expected[..39], &actual));
        kani::cover!(accepted);
        kani::cover!(!accepted);
    }

    #[kani::proof]
    fn gate_rejection_is_permanent() {
        let conclusion = match kani::any::<u8>() % 4 {
            0 => Conclusion::Success,
            1 => Conclusion::Failure,
            2 => Conclusion::Cancelled,
            3 => Conclusion::Skipped,
            _ => unreachable!(),
        };
        assert_eq!(GateState::Rejected.observe(conclusion), GateState::Rejected);
        assert!(!GateState::Empty.passed());
        assert_eq!(
            GateState::Empty.observe(conclusion).passed(),
            conclusion == Conclusion::Success
        );
        assert_eq!(
            GateState::Passed.observe(conclusion).passed(),
            conclusion == Conclusion::Success
        );
        kani::cover!(GateState::Empty.observe(conclusion).passed());
        kani::cover!(!GateState::Empty.observe(conclusion).passed());
    }

    #[kani::proof]
    #[kani::unwind(9)]
    fn gate_requires_every_selected_check() {
        let results: [u8; 8] = kani::any();
        let conclusions = results.map(|result| match result % 4 {
            0 => Conclusion::Success,
            1 => Conclusion::Failure,
            2 => Conclusion::Cancelled,
            3 => Conclusion::Skipped,
            _ => unreachable!(),
        });
        assert_eq!(
            gate(&conclusions),
            results.iter().all(|result| result % 4 == 0)
        );
        assert!(!gate(&[]));
        kani::cover!(gate(&conclusions));
        kani::cover!(conclusions[0] == Conclusion::Skipped);
    }

    #[kani::proof]
    #[kani::unwind(41)]
    fn commit_identity_rejects_any_non_hexadecimal_byte() {
        let bytes: [u8; 40] = kani::any();
        let expected = bytes.iter().all(|byte| {
            (b'0'..=b'9').contains(byte)
                || (b'a'..=b'f').contains(byte)
                || (b'A'..=b'F').contains(byte)
        });
        assert_eq!(exact_hash(&bytes, 40), expected);
        assert!(!exact_hash(&bytes, 39));
        assert!(!exact_hash(&bytes, 41));
        kani::cover!(exact_hash(&bytes, 40));
        kani::cover!(!exact_hash(&bytes, 40));
    }

    #[cfg(feature = "counterexample")]
    #[kani::proof]
    fn reject_empty_gate_probe() {
        assert!(gate(&[]));
    }
}
