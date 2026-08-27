//! Binary split layout for in-window terminal panes.

use alacritty_terminal::term::{MIN_COLUMNS, MIN_SCREEN_LINES};

/// Identifier for a leaf pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

impl PaneId {
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// Identifier for a split divider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SplitId(u64);

/// Split orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Top/bottom stack (`SplitDown`).
    Horizontal,
    /// Left/right columns (`SplitRight`).
    Vertical,
}

/// Keyboard focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Pixel rectangle in window coordinates (origin top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }

    pub fn inflate(self, slop: f32) -> Self {
        Self {
            x: self.x - slop,
            y: self.y - slop,
            width: self.width + slop * 2.,
            height: self.height + slop * 2.,
        }
    }

    pub fn center(self) -> (f32, f32) {
        (self.x + self.width / 2., self.y + self.height / 2.)
    }
}

/// Cell metrics used to snap splits to the grid.
#[derive(Debug, Clone, Copy)]
pub struct LayoutMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub bar_thickness: f32,
    pub hit_slop: f32,
}

impl LayoutMetrics {
    pub fn from_cell_size(cell_width: f32, cell_height: f32) -> Self {
        Self { cell_width, cell_height, bar_thickness: cell_width.max(4.), hit_slop: 4. }
    }
}

/// Laid-out leaf pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeafGeometry {
    pub id: PaneId,
    pub rect: Rect,
    pub columns: usize,
    pub screen_lines: usize,
}

/// Laid-out split bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarGeometry {
    pub id: SplitId,
    pub axis: Axis,
    pub rect: Rect,
}

/// Result of computing a layout tree.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutGeometry {
    pub leaves: Vec<LeafGeometry>,
    pub bars: Vec<BarGeometry>,
}

impl LayoutGeometry {
    pub fn leaf(&self, id: PaneId) -> Option<&LeafGeometry> {
        self.leaves.iter().find(|leaf| leaf.id == id)
    }
}

/// Hit-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Pane(PaneId),
    Bar { id: SplitId, axis: Axis },
}

enum Node {
    Leaf(PaneId),
    Split { id: SplitId, axis: Axis, ratio: f32, first: Box<Node>, second: Box<Node> },
}

/// Binary tree of panes.
pub struct Layout {
    root: Node,
    next_pane_id: u64,
    next_split_id: u64,
}

impl Layout {
    /// Create a layout with a single pane.
    pub fn new() -> (Self, PaneId) {
        let id = PaneId(1);
        (Self { root: Node::Leaf(id), next_pane_id: 2, next_split_id: 1 }, id)
    }

    pub fn is_single_pane(&self) -> bool {
        matches!(self.root, Node::Leaf(_))
    }

    /// Split `focused` along `axis`, returning the new pane id.
    pub fn split(&mut self, focused: PaneId, axis: Axis) -> Option<PaneId> {
        let new_id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let split_id = SplitId(self.next_split_id);
        self.next_split_id += 1;
        if self.root.split_leaf(focused, axis, new_id, split_id) { Some(new_id) } else { None }
    }

    /// Remove `id`, collapsing its parent split.
    ///
    /// Returns the sibling that should receive focus. `None` if this was the last pane.
    pub fn close(&mut self, id: PaneId) -> Option<PaneId> {
        if self.is_single_pane() {
            return None;
        }

        self.root.close(id)
    }

    pub fn compute(&self, bounds: Rect, metrics: LayoutMetrics) -> LayoutGeometry {
        let mut geometry = LayoutGeometry { leaves: Vec::new(), bars: Vec::new() };
        self.root.compute(bounds, metrics, &mut geometry);
        geometry
    }

    pub fn hit_test(&self, bounds: Rect, metrics: LayoutMetrics, x: f32, y: f32) -> Option<Hit> {
        let geometry = self.compute(bounds, metrics);
        for bar in &geometry.bars {
            if bar.rect.inflate(metrics.hit_slop).contains(x, y) {
                return Some(Hit::Bar { id: bar.id, axis: bar.axis });
            }
        }
        geometry.leaves.iter().find(|leaf| leaf.rect.contains(x, y)).map(|leaf| Hit::Pane(leaf.id))
    }

    /// Drag a split bar. Returns `true` if the ratio changed.
    pub fn drag_split(
        &mut self,
        id: SplitId,
        bounds: Rect,
        metrics: LayoutMetrics,
        pointer_x: f32,
        pointer_y: f32,
    ) -> bool {
        self.root.drag(id, bounds, metrics, pointer_x, pointer_y)
    }

    pub fn neighbor(
        &self,
        bounds: Rect,
        metrics: LayoutMetrics,
        focused: PaneId,
        direction: FocusDirection,
    ) -> Option<PaneId> {
        let geometry = self.compute(bounds, metrics);
        let current = geometry.leaf(focused)?;
        let (cx, cy) = current.rect.center();
        let mut best = None;
        let mut best_dist = f32::MAX;

        for leaf in &geometry.leaves {
            if leaf.id == focused {
                continue;
            }

            let (lx, ly) = leaf.rect.center();
            let dx = lx - cx;
            let dy = ly - cy;
            let overlap = match direction {
                FocusDirection::Left => {
                    dx < -0.5
                        && rects_overlap_1d(
                            current.rect.y,
                            current.rect.height,
                            leaf.rect.y,
                            leaf.rect.height,
                        )
                },
                FocusDirection::Right => {
                    dx > 0.5
                        && rects_overlap_1d(
                            current.rect.y,
                            current.rect.height,
                            leaf.rect.y,
                            leaf.rect.height,
                        )
                },
                FocusDirection::Up => {
                    dy < -0.5
                        && rects_overlap_1d(
                            current.rect.x,
                            current.rect.width,
                            leaf.rect.x,
                            leaf.rect.width,
                        )
                },
                FocusDirection::Down => {
                    dy > 0.5
                        && rects_overlap_1d(
                            current.rect.x,
                            current.rect.width,
                            leaf.rect.x,
                            leaf.rect.width,
                        )
                },
            };
            if !overlap {
                continue;
            }

            let dist = dx * dx + dy * dy;
            if dist < best_dist {
                best_dist = dist;
                best = Some(leaf.id);
            }
        }

        best
    }
}

fn rects_overlap_1d(a: f32, a_len: f32, b: f32, b_len: f32) -> bool {
    a < b + b_len && b < a + a_len
}

impl Node {
    fn split_leaf(
        &mut self,
        focused: PaneId,
        axis: Axis,
        new_id: PaneId,
        split_id: SplitId,
    ) -> bool {
        match self {
            Node::Leaf(id) if *id == focused => {
                *self = Node::Split {
                    id: split_id,
                    axis,
                    ratio: 0.5,
                    first: Box::new(Node::Leaf(focused)),
                    second: Box::new(Node::Leaf(new_id)),
                };
                true
            },
            Node::Leaf(_) => false,
            Node::Split { first, second, .. } => {
                first.split_leaf(focused, axis, new_id, split_id)
                    || second.split_leaf(focused, axis, new_id, split_id)
            },
        }
    }

    fn first_leaf(&self) -> PaneId {
        match self {
            Node::Leaf(id) => *id,
            Node::Split { first, .. } => first.first_leaf(),
        }
    }

    fn close(&mut self, id: PaneId) -> Option<PaneId> {
        match self {
            Node::Leaf(_) => None,
            Node::Split { first, second, .. } => {
                if let Node::Leaf(pid) = **first {
                    if pid == id {
                        let focus = second.first_leaf();
                        *self = mem_take_node(second);
                        return Some(focus);
                    }
                }
                if let Node::Leaf(pid) = **second {
                    if pid == id {
                        let focus = first.first_leaf();
                        *self = mem_take_node(first);
                        return Some(focus);
                    }
                }
                first.close(id).or_else(|| second.close(id))
            },
        }
    }

    fn compute(&self, bounds: Rect, metrics: LayoutMetrics, geometry: &mut LayoutGeometry) {
        match self {
            Node::Leaf(id) => {
                let columns = ((bounds.width / metrics.cell_width) as usize).max(MIN_COLUMNS);
                let screen_lines =
                    ((bounds.height / metrics.cell_height) as usize).max(MIN_SCREEN_LINES);
                geometry.leaves.push(LeafGeometry { id: *id, rect: bounds, columns, screen_lines });
            },
            Node::Split { id, axis, ratio, first, second } => {
                let (first_bounds, bar, second_bounds) = split_rect(bounds, *axis, *ratio, metrics);
                first.compute(first_bounds, metrics, geometry);
                geometry.bars.push(BarGeometry { id: *id, axis: *axis, rect: bar });
                second.compute(second_bounds, metrics, geometry);
            },
        }
    }

    fn drag(
        &mut self,
        id: SplitId,
        bounds: Rect,
        metrics: LayoutMetrics,
        pointer_x: f32,
        pointer_y: f32,
    ) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { id: split_id, axis, ratio, first, second } => {
                if *split_id == id {
                    let new_ratio =
                        ratio_from_pointer(bounds, *axis, metrics, pointer_x, pointer_y);
                    if (*ratio - new_ratio).abs() > f32::EPSILON {
                        *ratio = new_ratio;
                        return true;
                    }
                    return false;
                }

                let (first_bounds, _, second_bounds) = split_rect(bounds, *axis, *ratio, metrics);
                first.drag(id, first_bounds, metrics, pointer_x, pointer_y)
                    || second.drag(id, second_bounds, metrics, pointer_x, pointer_y)
            },
        }
    }
}

fn mem_take_node(node: &mut Box<Node>) -> Node {
    let dummy = Node::Leaf(PaneId(0));
    std::mem::replace(node.as_mut(), dummy)
}

fn min_primary(axis: Axis, metrics: LayoutMetrics) -> f32 {
    match axis {
        Axis::Vertical => MIN_COLUMNS as f32 * metrics.cell_width,
        Axis::Horizontal => MIN_SCREEN_LINES as f32 * metrics.cell_height,
    }
}

fn split_rect(bounds: Rect, axis: Axis, ratio: f32, metrics: LayoutMetrics) -> (Rect, Rect, Rect) {
    let bar = metrics.bar_thickness;
    match axis {
        Axis::Vertical => {
            let available = (bounds.width - bar).max(0.);
            let first_len =
                snapped_first_len(available, ratio, metrics.cell_width, min_primary(axis, metrics));
            let first = Rect { x: bounds.x, y: bounds.y, width: first_len, height: bounds.height };
            let bar_rect =
                Rect { x: bounds.x + first_len, y: bounds.y, width: bar, height: bounds.height };
            let second = Rect {
                x: bounds.x + first_len + bar,
                y: bounds.y,
                width: (available - first_len).max(0.),
                height: bounds.height,
            };
            (first, bar_rect, second)
        },
        Axis::Horizontal => {
            let available = (bounds.height - bar).max(0.);
            let first_len = snapped_first_len(
                available,
                ratio,
                metrics.cell_height,
                min_primary(axis, metrics),
            );
            let first = Rect { x: bounds.x, y: bounds.y, width: bounds.width, height: first_len };
            let bar_rect =
                Rect { x: bounds.x, y: bounds.y + first_len, width: bounds.width, height: bar };
            let second = Rect {
                x: bounds.x,
                y: bounds.y + first_len + bar,
                width: bounds.width,
                height: (available - first_len).max(0.),
            };
            (first, bar_rect, second)
        },
    }
}

fn snapped_first_len(available: f32, ratio: f32, cell: f32, min_size: f32) -> f32 {
    if available <= 0. {
        return 0.;
    }

    let min_size = min_size.min(available);
    let max_first = (available - min_size).max(min_size);
    let desired = (available * ratio.clamp(0., 1.)).clamp(min_size, max_first);
    let snapped = if cell > 0. { (desired / cell).floor() * cell } else { desired };
    snapped.clamp(min_size, max_first)
}

fn ratio_from_pointer(
    bounds: Rect,
    axis: Axis,
    metrics: LayoutMetrics,
    pointer_x: f32,
    pointer_y: f32,
) -> f32 {
    let bar = metrics.bar_thickness;
    let (available, pos, min_size, cell) = match axis {
        Axis::Vertical => (
            (bounds.width - bar).max(0.),
            pointer_x - bounds.x,
            min_primary(axis, metrics),
            metrics.cell_width,
        ),
        Axis::Horizontal => (
            (bounds.height - bar).max(0.),
            pointer_y - bounds.y,
            min_primary(axis, metrics),
            metrics.cell_height,
        ),
    };

    if available <= 0. {
        return 0.5;
    }

    let min_size = min_size.min(available);
    let min_ratio = min_size / available;
    let max_ratio = ((available - min_size) / available).max(min_ratio);
    let snapped_pos = if cell > 0. { (pos / cell).round() * cell } else { pos };
    (snapped_pos / available).clamp(min_ratio, max_ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> LayoutMetrics {
        LayoutMetrics { cell_width: 10., cell_height: 20., bar_thickness: 10., hit_slop: 4. }
    }

    fn bounds() -> Rect {
        Rect { x: 0., y: 0., width: 200., height: 200. }
    }

    #[test]
    fn nested_split_geometry() {
        let (mut layout, root) = Layout::new();
        let right = layout.split(root, Axis::Vertical).unwrap();
        let bottom = layout.split(root, Axis::Horizontal).unwrap();

        let geometry = layout.compute(bounds(), metrics());
        assert_eq!(geometry.leaves.len(), 3);
        assert_eq!(geometry.bars.len(), 2);

        let left_top = geometry.leaf(root).unwrap();
        let left_bottom = geometry.leaf(bottom).unwrap();
        let right_pane = geometry.leaf(right).unwrap();

        assert!(left_top.rect.x < right_pane.rect.x);
        assert!(left_top.rect.y < left_bottom.rect.y);
        assert_eq!(left_top.columns + right_pane.columns, 19);
        assert!((left_top.columns as i32 - right_pane.columns as i32).abs() <= 1);
        assert!(left_top.screen_lines >= MIN_SCREEN_LINES);
        assert!(left_bottom.screen_lines >= MIN_SCREEN_LINES);
    }

    #[test]
    fn hit_test_leaf_vs_bar() {
        let (mut layout, root) = Layout::new();
        layout.split(root, Axis::Vertical).unwrap();
        let geometry = layout.compute(bounds(), metrics());
        let bar = geometry.bars[0];

        assert!(matches!(layout.hit_test(bounds(), metrics(), 20., 20.), Some(Hit::Pane(_))));
        assert_eq!(
            layout.hit_test(bounds(), metrics(), bar.rect.x + 1., 50.),
            Some(Hit::Bar { id: bar.id, axis: Axis::Vertical })
        );
        // Slop around the bar still hits the divider.
        assert_eq!(
            layout.hit_test(bounds(), metrics(), bar.rect.x - 2., 50.),
            Some(Hit::Bar { id: bar.id, axis: Axis::Vertical })
        );
    }

    #[test]
    fn drag_clamps_at_min_cols() {
        let (mut layout, root) = Layout::new();
        layout.split(root, Axis::Vertical).unwrap();
        let geometry = layout.compute(bounds(), metrics());
        let bar_id = geometry.bars[0].id;

        layout.drag_split(bar_id, bounds(), metrics(), 0., 0.);
        let geometry = layout.compute(bounds(), metrics());
        assert!(geometry.leaves.iter().all(|leaf| leaf.columns >= MIN_COLUMNS));

        layout.drag_split(bar_id, bounds(), metrics(), 200., 0.);
        let geometry = layout.compute(bounds(), metrics());
        assert!(geometry.leaves.iter().all(|leaf| leaf.columns >= MIN_COLUMNS));
    }

    #[test]
    fn close_collapses_to_single_leaf() {
        let (mut layout, root) = Layout::new();
        let right = layout.split(root, Axis::Vertical).unwrap();
        let focus = layout.close(right).unwrap();
        assert_eq!(focus, root);
        assert!(layout.is_single_pane());

        let geometry = layout.compute(bounds(), metrics());
        assert_eq!(geometry.leaves.len(), 1);
        assert!(geometry.bars.is_empty());
        assert_eq!(geometry.leaves[0].id, root);
        assert!(layout.close(root).is_none());
    }

    #[test]
    fn neighbor_focus() {
        let (mut layout, root) = Layout::new();
        let right = layout.split(root, Axis::Vertical).unwrap();
        assert_eq!(layout.neighbor(bounds(), metrics(), root, FocusDirection::Right), Some(right));
        assert_eq!(layout.neighbor(bounds(), metrics(), right, FocusDirection::Left), Some(root));
        assert_eq!(layout.neighbor(bounds(), metrics(), root, FocusDirection::Left), None);
    }
}
