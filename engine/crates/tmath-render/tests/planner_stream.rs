use tmath_render::{
    content_hash, Limits, PlacementPlanner, PlanOp, RenderOptions, Revision, StreamSplitter,
};

fn inputs(revision: &Revision) -> Vec<([u8; 32], u32, u32)> {
    revision
        .blocks
        .iter()
        .map(|block| {
            (
                content_hash(block, &RenderOptions::default()),
                320,
                u32::try_from(block.source.len()).expect("test source length must fit in u32"),
            )
        })
        .collect()
}

#[test]
fn splitter_revisions_feed_append_tail_replace_and_direct_interior_edit_plans() {
    let mut splitter = StreamSplitter::new(Limits::default());
    let mut planner = PlacementPlanner::new();

    let first = splitter.push(b"Alpha.\n\n").unwrap();
    assert!(matches!(
        planner.plan(&inputs(&first)).ops.as_slice(),
        [PlanOp::Append { .. }]
    ));

    let second = splitter.push(b"Bravo").unwrap();
    assert!(matches!(
        planner.plan(&inputs(&second)).ops.as_slice(),
        [PlanOp::Keep { .. }, PlanOp::Append { .. }]
    ));

    let third = splitter.push(b" grows.\n\nCharlie.\n\n").unwrap();
    assert!(matches!(
        planner.plan(&inputs(&third)).ops.as_slice(),
        [
            PlanOp::Keep { .. },
            PlanOp::Replace { .. },
            PlanOp::Append { .. }
        ]
    ));
    let charlie_id = planner.blocks()[2].id;

    // StreamSplitter is append-only, so a full interior edit is fed directly
    // to the planner here. CLI/watch wiring for full revisions lands in T3-204.
    let mut edited = inputs(&third);
    edited[1].0 = [0x5a; 32];
    edited[1].1 += 10;
    edited[1].2 += 20;
    let plan = planner.plan(&edited);

    assert!(matches!(
        plan.ops.as_slice(),
        [
            PlanOp::Keep { .. },
            PlanOp::Replace { .. },
            PlanOp::Keep { id }
        ] if *id == charlie_id
    ));
    assert_eq!(plan.reanchor_from, Some(1));
}
