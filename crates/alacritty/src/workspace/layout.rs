//! Binary split layout. Pane IDs stay stable when siblings are closed or resized.

pub type PaneId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug)]
pub enum PaneCommand {
    Split(Axis),
    Next,
    Previous,
    Close,
    CloseTab,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
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
}

#[derive(Debug)]
pub enum Layout {
    Leaf(PaneId),
    Split { axis: Axis, ratio: f32, first: Box<Layout>, second: Box<Layout> },
}

impl Layout {
    pub fn split(&mut self, target: PaneId, new: PaneId, axis: Axis) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(Self::Leaf(target)),
                    second: Box::new(Self::Leaf(new)),
                };
                true
            },
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.split(target, new, axis) || second.split(target, new, axis)
            },
        }
    }

    /// Remove a leaf and collapse its now-empty parent. The final leaf is kept.
    pub fn remove(&mut self, target: PaneId) -> bool {
        match self {
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                if matches!(**first, Self::Leaf(id) if id == target) {
                    *self = std::mem::replace(&mut **second, Self::Leaf(target));
                    true
                } else if matches!(**second, Self::Leaf(id) if id == target) {
                    *self = std::mem::replace(&mut **first, Self::Leaf(target));
                    true
                } else {
                    first.remove(target) || second.remove(target)
                }
            },
        }
    }

    pub fn rects(&self, bounds: Rect, gap: f32) -> Vec<(PaneId, Rect)> {
        let mut result = Vec::new();
        self.visit(bounds, gap, &mut result);
        result
    }

    fn children(bounds: Rect, axis: Axis, ratio: f32, gap: f32) -> (Rect, Rect, Rect) {
        let length = match axis {
            Axis::Horizontal => bounds.width,
            Axis::Vertical => bounds.height,
        };
        let gap = gap.min((length - 2.).max(0.));
        let available = (length - gap).max(0.);
        let first = (available * ratio).floor().clamp(0., available);
        let second = available - first;
        match axis {
            Axis::Horizontal => (
                Rect { width: first, ..bounds },
                Rect { x: bounds.x + first + gap, width: second, ..bounds },
                Rect { x: bounds.x + first, width: gap, ..bounds },
            ),
            Axis::Vertical => (
                Rect { height: first, ..bounds },
                Rect { y: bounds.y + first + gap, height: second, ..bounds },
                Rect { y: bounds.y + first, height: gap, ..bounds },
            ),
        }
    }

    fn visit(&self, bounds: Rect, gap: f32, result: &mut Vec<(PaneId, Rect)>) {
        match self {
            Self::Leaf(id) => result.push((*id, bounds)),
            Self::Split { axis, ratio, first, second } => {
                let (a, b, _) = Self::children(bounds, *axis, *ratio, gap);
                first.visit(a, gap, result);
                second.visit(b, gap, result);
            },
        }
    }

    /// A path identifies a divider independently of the pointer's later position.
    pub fn divider_at(
        &self,
        bounds: Rect,
        gap: f32,
        hit_padding: f32,
        x: f32,
        y: f32,
    ) -> Option<(Vec<bool>, Axis)> {
        if !bounds.contains(x, y) {
            return None;
        }
        match self {
            Self::Leaf(_) => None,
            Self::Split { axis, ratio, first, second } => {
                let (a, b, mut divider) = Self::children(bounds, *axis, *ratio, gap);
                // Keep hairline dividers easy to grab without reserving a wide gutter.
                match axis {
                    Axis::Horizontal => {
                        divider.x -= hit_padding;
                        divider.width += 2. * hit_padding;
                    },
                    Axis::Vertical => {
                        divider.y -= hit_padding;
                        divider.height += 2. * hit_padding;
                    },
                }
                if divider.contains(x, y) {
                    return Some((Vec::new(), *axis));
                }
                let (child, rect, side) =
                    if a.contains(x, y) { (first, a, false) } else { (second, b, true) };
                let (mut path, axis) = child.divider_at(rect, gap, hit_padding, x, y)?;
                path.insert(0, side);
                Some((path, axis))
            },
        }
    }

    fn minimum(&self, gap: f32, minimum: (f32, f32)) -> (f32, f32) {
        match self {
            Self::Leaf(_) => minimum,
            Self::Split { axis, first, second, .. } => {
                let a = first.minimum(gap, minimum);
                let b = second.minimum(gap, minimum);
                match axis {
                    Axis::Horizontal => (a.0 + b.0 + gap, a.1.max(b.1)),
                    Axis::Vertical => (a.0.max(b.0), a.1 + b.1 + gap),
                }
            },
        }
    }

    pub fn drag(
        &mut self,
        path: &[bool],
        bounds: Rect,
        gap: f32,
        point: (f32, f32),
        minimum: (f32, f32),
    ) {
        let Self::Split { axis, ratio, first, second } = self else { return };
        if let Some((side, rest)) = path.split_first() {
            let (a, b, _) = Self::children(bounds, *axis, *ratio, gap);
            if *side {
                second.drag(rest, b, gap, point, minimum);
            } else {
                first.drag(rest, a, gap, point, minimum);
            }
            return;
        }
        let a = first.minimum(gap, minimum);
        let b = second.minimum(gap, minimum);
        let (position, length, low, high) = match axis {
            Axis::Horizontal => (point.0 - bounds.x, bounds.width - gap, a.0, b.0),
            Axis::Vertical => (point.1 - bounds.y, bounds.height - gap, a.1, b.1),
        };
        if length >= low + high {
            *ratio = (position / length).clamp(low / length, 1. - high / length);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect { width: 1000., height: 600., ..Rect::default() }
    }

    #[test]
    fn nested_splits_cover_bounds_and_collapse_on_close() {
        let mut layout = Layout::Leaf(0);
        assert!(layout.split(0, 1, Axis::Horizontal));
        assert!(layout.split(1, 2, Axis::Vertical));
        let rects = layout.rects(bounds(), 4.);
        assert_eq!(rects[0].1, Rect { width: 498., height: 600., ..Rect::default() });
        assert_eq!(rects[2].1, Rect { x: 502., y: 302., width: 498., height: 298. });
        assert!(layout.remove(1));
        assert_eq!(layout.rects(bounds(), 4.)[1].1.height, 600.);
        assert!(layout.remove(0));
        assert_eq!(layout.rects(bounds(), 4.), vec![(2, bounds())]);
        assert!(!layout.remove(2));
    }

    #[test]
    fn divider_drag_respects_nested_minimums() {
        let mut layout = Layout::Leaf(0);
        layout.split(0, 1, Axis::Horizontal);
        layout.split(1, 2, Axis::Horizontal);
        let (path, axis) = layout.divider_at(bounds(), 4., 0., 499., 50.).unwrap();
        assert_eq!(axis, Axis::Horizontal);
        layout.drag(&path, bounds(), 4., (990., 50.), (100., 50.));
        let rects = layout.rects(bounds(), 4.);
        assert!(rects.iter().all(|(_, r)| r.width >= 100.));
        assert_eq!(rects[0].1.width, 792.);
        assert!(layout.divider_at(bounds(), 4., 0., 20., 20.).is_none());
    }

    #[test]
    fn hairline_dividers_have_a_wider_drag_target() {
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let mut layout = Layout::Leaf(0);
            layout.split(0, 1, axis);
            let rects = layout.rects(bounds(), 1.);
            let (edge, next) = match axis {
                Axis::Horizontal => (rects[0].1.width, rects[1].1.x),
                Axis::Vertical => (rects[0].1.height, rects[1].1.y),
            };
            assert_eq!(next - edge, 1.);
            for offset in [-3., -1., 0., 1., 3.] {
                let (x, y) = match axis {
                    Axis::Horizontal => (edge + offset, 50.),
                    Axis::Vertical => (50., edge + offset),
                };
                assert_eq!(layout.divider_at(bounds(), 1., 3., x, y), Some((vec![], axis)));
            }
            let (x, y) = match axis {
                Axis::Horizontal => (edge - 4., 50.),
                Axis::Vertical => (50., edge - 4.),
            };
            assert!(layout.divider_at(bounds(), 1., 3., x, y).is_none());
        }
    }

    #[test]
    fn nested_divider_hit_area_stays_inside_its_panes() {
        let mut layout = Layout::Leaf(0);
        layout.split(0, 1, Axis::Horizontal);
        layout.split(1, 2, Axis::Vertical);
        let hit = layout.divider_at(bounds(), 1., 3., 750., 297.);
        assert_eq!(hit, Some((vec![true], Axis::Vertical)));
        assert!(layout.divider_at(bounds(), 1., 3., 250., 297.).is_none());
        assert!(layout.divider_at(bounds(), 1., 3., 1001., 297.).is_none());
        assert!(layout.divider_at(bounds(), 1., 3., 499., -1.).is_none());
    }

    #[test]
    fn closing_a_nested_branch_preserves_sibling_ids_and_order() {
        let mut layout = Layout::Leaf(0);
        layout.split(0, 1, Axis::Horizontal);
        layout.split(1, 2, Axis::Vertical);
        layout.split(0, 3, Axis::Vertical);
        assert!(!layout.remove(99));
        assert!(layout.remove(1));
        assert_eq!(layout.rects(bounds(), 4.).iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![
            0, 3, 2
        ]);
        assert!(layout.remove(3));
        assert!(layout.remove(2));
        assert_eq!(layout.rects(bounds(), 4.), vec![(0, bounds())]);
    }

    #[test]
    fn extremely_small_window_keeps_rectangles_inside_bounds() {
        let mut layout = Layout::Leaf(0);
        for id in 1..10 {
            layout.split(id - 1, id, Axis::Horizontal);
        }
        let bounds = Rect { width: 2., height: 1., ..Rect::default() };
        for (_, rect) in layout.rects(bounds, 4.) {
            assert!(rect.x >= 0. && rect.width >= 0. && rect.x + rect.width <= bounds.width);
            assert_eq!(rect.height, bounds.height);
        }
    }
}
