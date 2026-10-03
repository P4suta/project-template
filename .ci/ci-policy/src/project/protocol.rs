use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Phase {
    Commit,
    Development,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Cache {
    Content,
    Always,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    NotSelected,
    OtherPlatform,
    Run,
    Reuse,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(kani, derive(kani::Arbitrary))]
pub enum Completion {
    Running,
    Success,
    Failure,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Wait,
    Accept,
    Reject,
    Timeout,
    Error,
}

pub fn progress(completion: Completion, expired: bool) -> Progress {
    match (completion, expired) {
        (Completion::Error, _) => Progress::Error,
        (Completion::Failure, _) => Progress::Reject,
        (Completion::Running | Completion::Success, true) => Progress::Timeout,
        (Completion::Running, false) => Progress::Wait,
        (Completion::Success, false) => Progress::Accept,
    }
}

pub fn decide(
    phase: Phase,
    required: Phase,
    applicable: bool,
    cache: Cache,
    exact_success: bool,
) -> Decision {
    if phase == Phase::Commit && required == Phase::Development {
        Decision::NotSelected
    } else if !applicable {
        Decision::OtherPlatform
    } else if cache == Cache::Content && exact_success {
        Decision::Reuse
    } else {
        Decision::Run
    }
}

pub fn budget_limit(phase: Phase) -> u64 {
    match phase {
        Phase::Commit => 30,
        Phase::Development => 1800,
    }
}

pub fn budget_valid(phase: Phase, seconds: u64) -> bool {
    seconds != 0 && seconds <= budget_limit(phase)
}

pub struct Budget {
    remaining: u64,
}

impl Budget {
    pub fn new(phase: Phase) -> Self {
        Self {
            remaining: budget_limit(phase),
        }
    }

    pub fn reserve(&mut self, seconds: u64) -> bool {
        if seconds == 0 || seconds > self.remaining {
            false
        } else {
            self.remaining -= seconds;
            true
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    Empty,
    Covered,
    Missing,
}

impl Coverage {
    pub fn observe(self, covered: bool) -> Self {
        match (self, covered) {
            (Self::Empty | Self::Covered, true) => Self::Covered,
            (Self::Missing, _) | (_, false) => Self::Missing,
        }
    }
    pub fn complete(self) -> bool {
        self == Self::Covered
    }
}

#[cfg(kani)]
mod proofs {
    use super::{
        Budget, Cache, Completion, Coverage, Decision, Phase, Progress, budget_limit, budget_valid,
        decide, progress,
    };

    #[kani::proof]
    fn reuse_requires_applicable_exact_success() {
        let phase: Phase = kani::any();
        let required: Phase = kani::any();
        let applicable: bool = kani::any();
        let cache: Cache = kani::any();
        let exact_success: bool = kani::any();
        let decision = decide(phase, required, applicable, cache, exact_success);
        if decision == Decision::Reuse {
            assert!(applicable && cache == Cache::Content && exact_success);
            assert!(phase == Phase::Development || required == Phase::Commit);
        }
        if applicable && (phase == Phase::Development || required == Phase::Commit) {
            assert_eq!(
                decision == Decision::Reuse,
                cache == Cache::Content && exact_success
            );
            assert!(matches!(decision, Decision::Run | Decision::Reuse));
        }
        kani::cover!(decision == Decision::Reuse);
        kani::cover!(decision == Decision::Run);
        kani::cover!(decision == Decision::OtherPlatform);
        kani::cover!(decision == Decision::NotSelected);
    }

    #[kani::proof]
    fn uncovered_or_empty_ci_cannot_complete() {
        let first: bool = kani::any();
        let second: bool = kani::any();
        let coverage = Coverage::Empty.observe(first).observe(second);
        assert_eq!(coverage.complete(), first && second);
        assert!(!Coverage::Empty.complete());
        assert!(!coverage.observe(false).observe(true).complete());
        kani::cover!(coverage.complete());
        kani::cover!(!coverage.complete());
    }

    #[kani::proof]
    fn automatic_checks_are_bounded_by_their_phase() {
        let phase: Phase = kani::any();
        let seconds: u64 = kani::any();
        let accepted = budget_valid(phase, seconds);
        assert_eq!(
            accepted,
            seconds >= 1 && seconds <= if phase == Phase::Commit { 30 } else { 1800 }
        );
        kani::cover!(accepted);
        kani::cover!(!accepted);
    }

    #[kani::proof]
    fn cumulative_budget_cannot_overflow_or_expand() {
        let phase: Phase = kani::any();
        let remaining: u64 = kani::any();
        kani::assume(remaining <= budget_limit(phase));
        let seconds: u64 = kani::any();
        let mut budget = Budget { remaining };
        let accepted = budget.reserve(seconds);
        assert_eq!(accepted, seconds != 0 && seconds <= remaining);
        assert_eq!(
            budget.remaining,
            if accepted {
                remaining - seconds
            } else {
                remaining
            }
        );
        assert!(budget.remaining <= budget_limit(phase));
        assert_eq!(Budget::new(phase).remaining, budget_limit(phase));
        kani::cover!(accepted);
        kani::cover!(!accepted);
        kani::cover!(accepted && budget.remaining == 0);
    }

    #[kani::proof]
    fn completion_requires_success_within_the_budget() {
        let completion: Completion = kani::any();
        let expired = kani::any();
        let result = progress(completion, expired);
        assert_eq!(
            result == Progress::Accept,
            completion == Completion::Success && !expired
        );
        assert_eq!(
            result == Progress::Wait,
            completion == Completion::Running && !expired
        );
        assert_eq!(
            result == Progress::Timeout,
            matches!(completion, Completion::Running | Completion::Success) && expired
        );
        assert_eq!(
            result == Progress::Reject,
            completion == Completion::Failure
        );
        assert_eq!(result == Progress::Error, completion == Completion::Error);
        kani::cover!(result == Progress::Accept);
        kani::cover!(result == Progress::Timeout);
        kani::cover!(result == Progress::Reject);
        kani::cover!(result == Progress::Error);
        kani::cover!(result == Progress::Wait);
    }
}
