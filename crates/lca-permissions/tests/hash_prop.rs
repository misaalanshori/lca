//! Property: the proposal-set hash detects any single mutation, so an edit
//! buried in a merge always re-prompts (ADR-0006, testing plan section 8).

use std::collections::BTreeMap;

use lca_permissions::proposal_hash;
use proptest::prelude::*;

fn arb_proposals() -> impl Strategy<Value = BTreeMap<String, String>> {
    proptest::collection::btree_map("[a-z]{1,8}", "[a-z]{1,12}", 0..6)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_single_byte_change_changes_the_hash(
        proposals in arb_proposals(),
        key_index in 0usize..8,
        flip_value in 0usize..8,
    ) {
        prop_assume!(!proposals.is_empty());
        let mut mutated = proposals.clone();
        let key = mutated.keys().nth(key_index % mutated.len()).cloned().expect("non-empty");
        let value = mutated.get(&key).cloned().expect("present");
        // Flip one byte of the value (or insert a byte when there is none).
        let mut bytes = value.clone().into_bytes();
        let at = flip_value % (bytes.len() + 1);
        if at == bytes.len() {
            bytes.push(b'!');
        } else {
            bytes[at] ^= 0x20;
        }
        let new_value = String::from_utf8(bytes).expect("ascii mutation");
        prop_assume!(new_value != value);
        mutated.insert(key, new_value);
        prop_assert_ne!(proposal_hash(&proposals), proposal_hash(&mutated));
    }

    #[test]
    fn reordering_never_changes_the_hash(a in arb_proposals(), b in arb_proposals()) {
        // Both maps iterate in key order by construction; two equal maps
        // hash equal regardless of insertion order.
        let mut rebuilt = BTreeMap::new();
        for (k, v) in a.iter().rev() {
            rebuilt.insert(k.clone(), v.clone());
        }
        prop_assert_eq!(proposal_hash(&a), proposal_hash(&rebuilt));
        let _ = &b;
    }
}
