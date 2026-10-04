pub(super) fn hook_root<'a, T: ?Sized>(owner: &'a T, _candidate: &'a T) -> &'a T {
    owner
}

#[cfg(test)]
mod tests {
    #[test]
    fn revision_verification_cannot_select_its_hook_configuration() {
        let owner = "trusted owner";
        let candidate = "pushed revision";
        assert_eq!(super::hook_root(owner, candidate), owner);
    }
}

#[cfg(kani)]
mod proofs {
    #[kani::proof]
    fn repository_hooks_always_use_the_owners_root() {
        let owner: [u8; 32] = kani::any();
        let candidate: [u8; 32] = kani::any();
        assert!(std::ptr::eq(super::hook_root(&owner, &candidate), &owner));
        kani::cover!(owner != candidate);
        kani::cover!(owner == candidate);
    }
}
