/// Stable placement identity within one planner instance.
pub type BlockId = u64;

/// Current block placement metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedBlock {
    pub id: BlockId,
    pub hash: [u8; 32],
    pub width_px: u32,
    pub height_px: u32,
}

/// One ordered placement operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanOp {
    Keep {
        id: BlockId,
    },
    Append {
        block: PlannedBlock,
    },
    Replace {
        old_id: BlockId,
        block: PlannedBlock,
    },
    Remove {
        id: BlockId,
    },
}

/// Placement operations for one revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub ops: Vec<PlanOp>,
    pub reanchor_from: Option<usize>,
}

/// Pure state machine for mapping block revisions to placement operations.
pub struct PlacementPlanner {
    blocks: Vec<PlannedBlock>,
    next_id: BlockId,
}

impl PlacementPlanner {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            next_id: 1,
        }
    }

    pub fn plan(&mut self, revision_hashes_and_dims: &[([u8; 32], u32, u32)]) -> Plan {
        let previous = std::mem::take(&mut self.blocks);
        let shared_len = previous.len().min(revision_hashes_and_dims.len());
        let mut ops = Vec::with_capacity(
            revision_hashes_and_dims
                .len()
                .max(previous.len())
                .saturating_add(previous.len().saturating_sub(shared_len)),
        );
        let mut blocks = Vec::with_capacity(revision_hashes_and_dims.len());

        for (index, &(hash, width_px, height_px)) in revision_hashes_and_dims.iter().enumerate() {
            match previous.get(index) {
                Some(old) if old.hash == hash => {
                    blocks.push(old.clone());
                    ops.push(PlanOp::Keep { id: old.id });
                }
                Some(old) => {
                    let block = self.allocate(hash, width_px, height_px);
                    blocks.push(block.clone());
                    ops.push(PlanOp::Replace {
                        old_id: old.id,
                        block,
                    });
                }
                None => {
                    let block = self.allocate(hash, width_px, height_px);
                    blocks.push(block.clone());
                    ops.push(PlanOp::Append { block });
                }
            }
        }

        for old in previous[shared_len..].iter().rev() {
            ops.push(PlanOp::Remove { id: old.id });
        }

        let reanchor_from = ops.iter().position(|op| !matches!(op, PlanOp::Keep { .. }));
        self.blocks = blocks;

        Plan { ops, reanchor_from }
    }

    pub fn blocks(&self) -> &[PlannedBlock] {
        &self.blocks
    }

    fn allocate(&mut self, hash: [u8; 32], width_px: u32, height_px: u32) -> PlannedBlock {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("placement block id space exhausted");
        PlannedBlock {
            id,
            hash,
            width_px,
            height_px,
        }
    }
}

impl Default for PlacementPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(value: u8) -> [u8; 32] {
        [value; 32]
    }

    fn block(value: u8, height_px: u32) -> ([u8; 32], u32, u32) {
        (hash(value), 320, height_px)
    }

    #[test]
    fn append_only_reuses_prefix_ids_and_reanchors_at_each_append() {
        let mut planner = PlacementPlanner::new();

        let first = planner.plan(&[block(1, 10)]);
        let a_id = planner.blocks()[0].id;
        assert_eq!(
            first,
            Plan {
                ops: vec![PlanOp::Append {
                    block: planner.blocks()[0].clone(),
                }],
                reanchor_from: Some(0),
            }
        );

        let second = planner.plan(&[block(1, 10), block(2, 20)]);
        let b_id = planner.blocks()[1].id;
        assert_eq!(
            second,
            Plan {
                ops: vec![
                    PlanOp::Keep { id: a_id },
                    PlanOp::Append {
                        block: planner.blocks()[1].clone(),
                    },
                ],
                reanchor_from: Some(1),
            }
        );

        let third = planner.plan(&[block(1, 10), block(2, 20), block(3, 30)]);
        assert_eq!(
            third,
            Plan {
                ops: vec![
                    PlanOp::Keep { id: a_id },
                    PlanOp::Keep { id: b_id },
                    PlanOp::Append {
                        block: planner.blocks()[2].clone(),
                    },
                ],
                reanchor_from: Some(2),
            }
        );
        assert_eq!(planner.blocks()[0].id, a_id);
        assert_eq!(planner.blocks()[1].id, b_id);
    }

    #[test]
    fn tail_replace_allocates_a_new_id_and_keeps_the_prefix() {
        let mut planner = PlacementPlanner::new();
        planner.plan(&[block(1, 10), block(2, 20)]);
        let a_id = planner.blocks()[0].id;
        let old_b_id = planner.blocks()[1].id;

        let plan = planner.plan(&[block(1, 10), block(3, 20)]);
        let new_b = planner.blocks()[1].clone();

        assert_eq!(
            plan,
            Plan {
                ops: vec![
                    PlanOp::Keep { id: a_id },
                    PlanOp::Replace {
                        old_id: old_b_id,
                        block: new_b.clone(),
                    },
                ],
                reanchor_from: Some(1),
            }
        );
        assert_ne!(new_b.id, old_b_id);
    }

    #[test]
    fn interior_edit_keeps_the_unchanged_suffix_identity() {
        let mut planner = PlacementPlanner::new();
        planner.plan(&[block(1, 10), block(2, 20), block(3, 30)]);
        let [a, old_b, c] = planner.blocks() else {
            panic!("expected three planned blocks");
        };
        let (a_id, old_b_id, c_id) = (a.id, old_b.id, c.id);

        let plan = planner.plan(&[block(1, 10), block(4, 20), block(3, 30)]);

        assert_eq!(
            plan.ops,
            vec![
                PlanOp::Keep { id: a_id },
                PlanOp::Replace {
                    old_id: old_b_id,
                    block: planner.blocks()[1].clone(),
                },
                PlanOp::Keep { id: c_id },
            ]
        );
        assert_eq!(plan.reanchor_from, Some(1));
        assert_eq!(planner.blocks()[2].id, c_id);
        assert_eq!(planner.blocks()[2].hash, hash(3));
    }

    #[test]
    fn interior_height_change_still_keeps_the_unchanged_suffix_op() {
        let mut planner = PlacementPlanner::new();
        planner.plan(&[block(1, 10), block(2, 20), block(3, 30)]);
        let old_b_id = planner.blocks()[1].id;
        let c = planner.blocks()[2].clone();

        let plan = planner.plan(&[block(1, 10), block(4, 80), block(3, 30)]);

        assert!(matches!(
            &plan.ops[1],
            PlanOp::Replace { old_id, .. } if *old_id == old_b_id
        ));
        assert_eq!(plan.ops[2], PlanOp::Keep { id: c.id });
        assert_eq!(plan.reanchor_from, Some(1));
        assert_eq!(planner.blocks()[2], c);
    }

    #[test]
    fn shrink_removes_trailing_blocks_last_first_after_keeps() {
        let mut planner = PlacementPlanner::new();
        planner.plan(&[block(1, 10), block(2, 20), block(3, 30)]);
        let [a, b, c] = planner.blocks() else {
            panic!("expected three planned blocks");
        };
        let (a_id, b_id, c_id) = (a.id, b.id, c.id);

        let plan = planner.plan(&[block(1, 10)]);

        assert_eq!(
            plan,
            Plan {
                ops: vec![
                    PlanOp::Keep { id: a_id },
                    PlanOp::Remove { id: c_id },
                    PlanOp::Remove { id: b_id },
                ],
                reanchor_from: Some(1),
            }
        );
    }

    #[test]
    fn duplicate_hashes_are_distinct_position_scoped_placements() {
        let mut planner = PlacementPlanner::new();
        planner.plan(&[block(1, 10), block(1, 10)]);
        let first_id = planner.blocks()[0].id;
        let second_id = planner.blocks()[1].id;
        assert_ne!(first_id, second_id);

        let shrink = planner.plan(&[block(1, 10)]);
        assert_eq!(
            shrink.ops,
            vec![
                PlanOp::Keep { id: first_id },
                PlanOp::Remove { id: second_id },
            ]
        );

        let grow = planner.plan(&[block(1, 10), block(1, 10)]);
        let replacement_second_id = planner.blocks()[1].id;
        assert_eq!(grow.ops[0], PlanOp::Keep { id: first_id });
        assert!(matches!(
            &grow.ops[1],
            PlanOp::Append { block } if block.id == replacement_second_id
        ));
        assert_ne!(replacement_second_id, second_id);
    }

    #[test]
    fn empty_and_no_change_revisions_have_no_reanchor() {
        let mut planner = PlacementPlanner::new();
        assert_eq!(
            planner.plan(&[]),
            Plan {
                ops: Vec::new(),
                reanchor_from: None,
            }
        );

        planner.plan(&[block(1, 10), block(2, 20)]);
        let ids = planner
            .blocks()
            .iter()
            .map(|block| block.id)
            .collect::<Vec<_>>();
        assert_eq!(
            planner.plan(&[block(1, 10), block(2, 20)]),
            Plan {
                ops: ids.into_iter().map(|id| PlanOp::Keep { id }).collect(),
                reanchor_from: None,
            }
        );
    }

    #[test]
    fn ids_are_monotonic_and_never_reused_after_removal() {
        let mut planner = PlacementPlanner::new();
        let mut allocated = Vec::new();

        for value in 1..=64 {
            let before = planner.blocks().to_vec();
            planner.plan(&[block(value, u32::from(value))]);
            let current_id = planner.blocks()[0].id;
            if before
                .first()
                .is_none_or(|previous| previous.id != current_id)
            {
                allocated.push(current_id);
            }
            planner.plan(&[]);
        }

        assert!(allocated.windows(2).all(|ids| ids[0] < ids[1]));
        allocated.sort_unstable();
        allocated.dedup();
        assert_eq!(allocated.len(), 64);
    }
}
