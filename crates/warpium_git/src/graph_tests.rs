use std::collections::HashSet;

use instant::Instant;

use super::*;

fn oid(n: u32) -> Oid {
    Oid::from_hex(&format!("{n:040x}")).unwrap()
}

fn seg(half: Half, from: u16, to: u16) -> (Half, u16, u16) {
    (half, from, to)
}

fn shape(row: &GraphRow) -> Vec<(Half, u16, u16)> {
    row.segments
        .iter()
        .map(|segment| (segment.half, segment.from, segment.to))
        .collect()
}

#[test]
fn linear_history_stays_in_one_lane() {
    let mut graph = GraphBuilder::new();
    let rows = [
        graph.push(oid(3), &[oid(2)]),
        graph.push(oid(2), &[oid(1)]),
        graph.push(oid(1), &[]),
    ];
    assert_eq!(shape(&rows[0]), [seg(Half::Bottom, 0, 0)]);
    assert_eq!(
        shape(&rows[1]),
        [seg(Half::Top, 0, 0), seg(Half::Bottom, 0, 0)]
    );
    assert_eq!(shape(&rows[2]), [seg(Half::Top, 0, 0)]);
    for row in &rows {
        assert_eq!(row.node, 0);
        assert_eq!(row.width, 1);
        assert_eq!(row.color, rows[0].color);
    }
    assert_eq!(graph.open_lanes(), 0);
}

#[test]
fn merge_opens_a_lane_that_joins_at_the_fork_point() {
    // M merges B into A; both come from C.
    let mut graph = GraphBuilder::new();
    let merge = graph.push(oid(4), &[oid(3), oid(2)]);
    assert_eq!(merge.node, 0);
    assert_eq!(
        shape(&merge),
        [seg(Half::Bottom, 0, 0), seg(Half::Bottom, 0, 1)]
    );
    assert_ne!(merge.segments[0].color, merge.segments[1].color);

    let a = graph.push(oid(3), &[oid(1)]);
    assert_eq!(a.node, 0);
    assert_eq!(
        shape(&a),
        [
            seg(Half::Top, 0, 0),
            seg(Half::Top, 1, 1),
            seg(Half::Bottom, 1, 1),
            seg(Half::Bottom, 0, 0),
        ]
    );

    let b = graph.push(oid(2), &[oid(1)]);
    assert_eq!(b.node, 1);
    assert_eq!(b.color, merge.segments[1].color);

    let c = graph.push(oid(1), &[]);
    assert_eq!(c.node, 0);
    assert_eq!(shape(&c), [seg(Half::Top, 0, 0), seg(Half::Top, 1, 0)]);
    assert_eq!(graph.open_lanes(), 0);
}

#[test]
fn branch_tips_open_lanes_on_the_right() {
    // Two tips (5 and 4) on top of 3.
    let mut graph = GraphBuilder::new();
    let first = graph.push(oid(5), &[oid(3)]);
    let second = graph.push(oid(4), &[oid(3)]);
    assert_eq!(first.node, 0);
    assert_eq!(second.node, 1);
    assert_eq!(
        shape(&second),
        [
            seg(Half::Top, 0, 0),
            seg(Half::Bottom, 0, 0),
            seg(Half::Bottom, 1, 1)
        ]
    );
    assert_eq!(second.width, 2);
    let base = graph.push(oid(3), &[]);
    assert_eq!(shape(&base), [seg(Half::Top, 0, 0), seg(Half::Top, 1, 0)]);
}

#[test]
fn lanes_compact_to_the_left_when_one_ends() {
    // Lanes wait for 3, 2 and 1; 2 is a root, so the lane of 1 moves left under it.
    let mut graph = GraphBuilder::new();
    graph.push(oid(9), &[oid(3)]);
    graph.push(oid(8), &[oid(2)]);
    graph.push(oid(7), &[oid(1)]);
    let root = graph.push(oid(2), &[]);
    assert_eq!(root.node, 1);
    assert_eq!(
        shape(&root),
        [
            seg(Half::Top, 0, 0),
            seg(Half::Top, 1, 1),
            seg(Half::Top, 2, 2),
            seg(Half::Bottom, 0, 0),
            seg(Half::Bottom, 2, 1),
        ]
    );
    assert_eq!(graph.open_lanes(), 2);
}

#[test]
fn merge_parent_joins_an_existing_lane() {
    // 9 waits for 2 in lane 0; the merge 8 has parents 3 and 2, and joins lane 0 for 2.
    let mut graph = GraphBuilder::new();
    graph.push(oid(9), &[oid(2)]);
    let merge = graph.push(oid(8), &[oid(3), oid(2)]);
    assert_eq!(merge.node, 1);
    assert_eq!(
        shape(&merge),
        [
            seg(Half::Top, 0, 0),
            seg(Half::Bottom, 0, 0),
            seg(Half::Bottom, 1, 1),
            seg(Half::Bottom, 1, 0),
        ]
    );
    assert_eq!(graph.open_lanes(), 2);
}

#[test]
fn octopus_merge_opens_free_slots_right_of_the_node() {
    let mut graph = GraphBuilder::new();
    let merge = graph.push(oid(9), &[oid(3), oid(2), oid(1)]);
    assert_eq!(
        shape(&merge),
        [
            seg(Half::Bottom, 0, 0),
            seg(Half::Bottom, 0, 1),
            seg(Half::Bottom, 0, 2),
        ]
    );
    assert_eq!(merge.width, 3);
}

#[test]
fn drawn_lanes_are_capped() {
    let mut graph = GraphBuilder::new();
    for n in 0..(MAX_DRAWN_LANES as u32 + 20) {
        graph.push(oid(1000 + n), &[oid(5000 + n)]);
    }
    let row = graph.push(oid(1), &[oid(2)]);
    assert_eq!(row.width as usize, MAX_DRAWN_LANES);
    assert!((row.node as usize) < MAX_DRAWN_LANES);
    assert!(row.segments.iter().all(|segment| {
        (segment.from as usize) < MAX_DRAWN_LANES && (segment.to as usize) < MAX_DRAWN_LANES
    }));
}

/// A synthetic history in `--date-order`: a mainline where every tenth commit merges a
/// two-commit feature branch forked four commits earlier.
fn synthetic_history(len: u32) -> Vec<(Oid, Vec<Oid>)> {
    let mut commits = Vec::with_capacity(len as usize + len as usize / 5);
    let mut side = len * 4;
    for main in (1..=len).rev() {
        if main == 1 {
            commits.push((oid(1), vec![]));
        } else if main % 10 == 0 && main > 5 {
            commits.push((oid(main), vec![oid(main - 1), oid(side)]));
            commits.push((oid(side), vec![oid(side + 1)]));
            commits.push((oid(side + 1), vec![oid(main - 4)]));
            side += 2;
        } else {
            commits.push((oid(main), vec![oid(main - 1)]));
        }
    }
    commits
}

/// Replays rows into a check that every edge leaving a row's bottom arrives at the next row's
/// top at the same lane, and that nodes sit on a lane that is drawn.
fn assert_rows_connect(rows: &[GraphRow]) {
    for pair in rows.windows(2) {
        let bottoms: HashSet<u16> = pair[0]
            .segments
            .iter()
            .filter(|segment| segment.half == Half::Bottom)
            .map(|segment| segment.to)
            .collect();
        let tops: HashSet<u16> = pair[1]
            .segments
            .iter()
            .filter(|segment| segment.half == Half::Top)
            .map(|segment| segment.from)
            .collect();
        assert_eq!(bottoms, tops);
    }
    for row in rows {
        assert!(row.node < row.width);
    }
}

#[test]
fn synthetic_history_connects() {
    let history = synthetic_history(3_000);
    let mut graph = GraphBuilder::new();
    let rows: Vec<GraphRow> = history
        .iter()
        .map(|(oid, parents)| graph.push(*oid, parents))
        .collect();
    assert_rows_connect(&rows);
    assert!(rows.iter().all(|row| row.width <= 4), "lanes stay few");
}

#[test]
fn lays_out_a_large_history_quickly() {
    let history = synthetic_history(300_000);
    let started = Instant::now();
    let mut graph = GraphBuilder::new();
    let mut segments = 0;
    for (oid, parents) in &history {
        segments += graph.push(*oid, parents).segments.len();
    }
    let elapsed = started.elapsed();
    assert!(segments > history.len());
    // Generous for debug builds on slow CI runners; a release build takes a few milliseconds.
    assert!(
        elapsed.as_secs_f64() < 10.,
        "laying out {} commits took {elapsed:?}",
        history.len()
    );
}

#[test]
fn segment_geometry() {
    let top = Segment {
        half: Half::Top,
        from: 0,
        to: 1,
        color: 0,
    };
    assert_eq!(segment_line(&top, 10., 20.), [(5., 0.), (15., 10.)]);
    let bottom = Segment {
        half: Half::Bottom,
        ..top
    };
    assert_eq!(segment_line(&bottom, 10., 20.), [(5., 10.), (15., 20.)]);
}

#[test]
fn stamps_cover_the_line() {
    let stamps = line_stamps((0., 0.), (10., 0.), 1.);
    assert_eq!(stamps.len(), 11);
    assert_eq!(stamps.first(), Some(&(0., 0.)));
    assert_eq!(stamps.last(), Some(&(10., 0.)));
    assert_eq!(line_stamps((3., 3.), (3., 3.), 1.).len(), 2);
}
