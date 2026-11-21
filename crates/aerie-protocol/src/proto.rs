//! Every protocol type, generated from `gen/protocol.json` (spec 06).
//!
//! Structs keep the field names of the schema in snake case; maps are lists
//! of pairs so their order survives a round trip; enums are newtypes over
//! their wire value with one constant per known value.

#![allow(clippy::all, non_snake_case, unused_parens, unused_variables)]

use crate::wire::{self, Wire};

include!(concat!(env!("OUT_DIR"), "/proto.rs"));

#[cfg(test)]
mod check_tests {
    use super::*;

    #[test]
    fn finds_duplicate_keys_and_bad_indexes() {
        let p = UpdateInteractions {
            max_id: 3,
            interactions: Some(vec![
                (0, Default::default()),
                (2, Default::default()),
                (2, Default::default()),
                (5, Default::default()),
            ]),
            ..Default::default()
        };
        let mut payload = Vec::new();
        p.encode(&mut payload).unwrap();
        let errs = check(66, &payload).unwrap().unwrap();
        assert!(
            errs.iter().any(|e| e.contains("duplicate key 2")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|e| e.contains("index 5 outside 0..3")),
            "{errs:?}"
        );
    }

    #[test]
    fn clean_registry_passes() {
        let p = UpdateInteractions {
            max_id: 2,
            interactions: Some(vec![(0, Default::default()), (1, Default::default())]),
            ..Default::default()
        };
        let mut payload = Vec::new();
        p.encode(&mut payload).unwrap();
        assert!(check(66, &payload).unwrap().unwrap().is_empty());
    }
}
