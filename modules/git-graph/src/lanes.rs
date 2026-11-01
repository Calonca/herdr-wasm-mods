//! Lays out the commit graph: which lane (column) each commit sits in and the lines that
//! run through its row. Commits come newest first in topological order, so every
//! commit's children are above it. Each lane waits for one commit (the parent of the
//! commit above it); a commit takes the leftmost lane waiting for it, other lanes waiting
//! for it too merge into its dot, its first parent continues its lane and further parents
//! (merges) join the lane already waiting for them or get one of their own. Lanes keep
//! their column until they end, so lines never jump sideways; freed columns are reused.

/// Where a line ends inside a row: its top edge, the commit's dot, or its bottom edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    Top,
    Dot,
    Bottom,
}

/// A line inside one row, from `(column, At)` to `(column, At)`, in a lane's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub from: (usize, At),
    pub to: (usize, At),
    /// Index into the palette, stable along a lane.
    pub color: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// The commit's column.
    pub column: usize,
    pub color: usize,
    pub lines: Vec<Line>,
    /// Columns in use in this row.
    pub width: usize,
}

/// The lane each column holds: the commit it waits for, and its colour.
type Lanes<'a> = Vec<Option<(&'a str, usize)>>;

/// The first free column from `from` on (a new one at the end if none is).
fn free(lanes: &mut Lanes, from: usize) -> usize {
    match (from..lanes.len()).find(|&i| lanes[i].is_none()) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

/// One row per commit. `parents[i]` are commit `i`'s parents' hashes; a parent missing
/// from `hashes` (beyond the loaded history) keeps its lane running off the bottom.
pub fn layout<'a>(hashes: &[&'a str], parents: &[Vec<&'a str>]) -> Vec<Row> {
    let mut lanes: Lanes<'a> = Vec::new();
    let mut next_color = 0;
    let mut rows = Vec::with_capacity(hashes.len());
    for (hash, parents) in hashes.iter().zip(parents) {
        let mut lines = Vec::new();
        let waiting: Vec<usize> = (0..lanes.len()).filter(|&i| lanes[i].is_some_and(|(h, _)| h == *hash)).collect();
        // A branch tip nothing waits for starts in the first free column.
        let (column, color) = match waiting.first() {
            Some(&i) => (i, lanes[i].map_or(0, |l| l.1)),
            None => {
                next_color += 1;
                (free(&mut lanes, 0), next_color - 1)
            }
        };
        for &i in &waiting {
            lines.push(Line { from: (i, At::Top), to: (column, At::Dot), color: lanes[i].map_or(0, |l| l.1) });
            lanes[i] = None;
        }
        // Lanes passing by.
        for (i, lane) in lanes.iter().enumerate() {
            if let Some((_, c)) = lane {
                lines.push(Line { from: (i, At::Top), to: (i, At::Bottom), color: *c });
            }
        }
        let width_above = lanes.len();
        for (n, parent) in parents.iter().enumerate() {
            // The first parent continues this lane (lanes waiting for the same commit meet
            // at its dot); another parent a lane already waits for joins that lane.
            if n > 0
                && let Some(i) = (0..lanes.len()).find(|&i| lanes[i].is_some_and(|(h, _)| h == *parent))
            {
                lines.push(Line { from: (column, At::Dot), to: (i, At::Bottom), color: lanes[i].map_or(0, |l| l.1) });
                continue;
            }
            let (i, c) = if n == 0 {
                (column, color)
            } else {
                next_color += 1;
                (free(&mut lanes, column + 1), next_color - 1)
            };
            lanes[i] = Some((parent, c));
            lines.push(Line { from: (column, At::Dot), to: (i, At::Bottom), color: c });
        }
        let width = width_above.max(lanes.len()).max(column + 1);
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(Row { column, color, lines, width });
    }
    rows
}
