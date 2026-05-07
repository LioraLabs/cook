//! Camera + viewport blit. See design spec §Camera.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::render::layout::{Layout, PlacedNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Camera {
    pub x: i32,
    pub y: i32,
}

impl Camera {
    pub fn origin() -> Self {
        Self { x: 0, y: 0 }
    }

    pub fn center_on(node: &PlacedNode, pane: Rect) -> Self {
        let cx = node.x as i32 + node.w as i32 / 2 - pane.width as i32 / 2;
        let cy = node.y as i32 + node.h as i32 / 2 - pane.height as i32 / 2;
        Self { x: cx, y: cy }
    }

    pub fn pan(&self, dx: i32, dy: i32, layout: &Layout, pane: Rect) -> Self {
        let max_x = (layout.canvas_w as i32 - pane.width as i32).max(0);
        let max_y = (layout.canvas_h as i32 - pane.height as i32).max(0);
        Self {
            x: (self.x + dx).clamp(0, max_x),
            y: (self.y + dy).clamp(0, max_y),
        }
    }

    pub fn auto_fit(layout: &Layout, pane: Rect) -> Self {
        let mid_x = (layout.canvas_w as i32 - pane.width as i32) / 2;
        let mid_y = (layout.canvas_h as i32 - pane.height as i32) / 2;
        Self { x: mid_x.max(0), y: mid_y.max(0) }
    }

    /// Align the bounding rect of the layout's placed nodes to the top
    /// of `pane`, centered horizontally. Returns the origin when the
    /// layout is empty. Negative x is allowed so a narrow bbox sits
    /// visually centered even when the camera coordinate is negative;
    /// the blit step renders only positive-coord cells.
    pub fn fit_bounds(layout: &Layout, pane: Rect) -> Self {
        let Some(first) = layout.nodes.first() else {
            return Self::origin();
        };
        let mut min_x = first.x as i32;
        let mut min_y = first.y as i32;
        let mut max_x = first.x as i32 + first.w as i32;
        for n in &layout.nodes[1..] {
            min_x = min_x.min(n.x as i32);
            min_y = min_y.min(n.y as i32);
            max_x = max_x.max(n.x as i32 + n.w as i32);
        }
        let cx = (min_x + max_x) / 2 - pane.width as i32 / 2;
        Self { x: cx, y: min_y }
    }

    /// Returns the side of the pane that contains the off-canvas selection,
    /// or None if the selection is fully visible.
    pub fn off_screen_side(&self, node: &PlacedNode, pane: Rect) -> Option<Side> {
        let nx = node.x as i32 - self.x;
        let ny = node.y as i32 - self.y;
        let nx_end = nx + node.w as i32;
        let ny_end = ny + node.h as i32;
        if nx_end <= 0 {
            Some(Side::Left)
        } else if nx >= pane.width as i32 {
            Some(Side::Right)
        } else if ny_end <= 0 {
            Some(Side::Top)
        } else if ny >= pane.height as i32 {
            Some(Side::Bottom)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// Blit the camera-clipped slice of `canvas` into `pane` of `dst`.
pub fn blit(canvas: &Buffer, camera: Camera, pane: Rect, dst: &mut Buffer) {
    for dy in 0..pane.height {
        for dx in 0..pane.width {
            let src_x = camera.x + dx as i32;
            let src_y = camera.y + dy as i32;
            if src_x < 0
                || src_y < 0
                || src_x >= canvas.area.width as i32
                || src_y >= canvas.area.height as i32
            {
                continue;
            }
            let src_cell = canvas.cell((src_x as u16, src_y as u16));
            if let (Some(src), Some(dst_cell)) =
                (src_cell, dst.cell_mut((pane.x + dx, pane.y + dy)))
            {
                *dst_cell = src.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::layout::{EdgeRoute, Layout, PlacedNode};

    fn layout_500x200() -> Layout {
        Layout {
            nodes: vec![PlacedNode {
                id: "n".into(),
                kind: "unit".into(),
                label: "n".into(),
                x: 100,
                y: 50,
                w: 22,
                h: 3,
                discovered: None,
            }],
            edges: vec![] as Vec<EdgeRoute>,
            canvas_w: 500,
            canvas_h: 200,
        }
    }

    #[test]
    fn center_on_centers_node_in_pane() {
        let l = layout_500x200();
        let pane = Rect::new(0, 0, 80, 24);
        let cam = Camera::center_on(&l.nodes[0], pane);
        // node center is (100+11, 50+1) = (111, 51); pane half is (40, 12).
        assert_eq!(cam.x, 71);
        assert_eq!(cam.y, 39);
    }

    #[test]
    fn pan_clamps_to_canvas_bounds() {
        let l = layout_500x200();
        let pane = Rect::new(0, 0, 80, 24);
        let cam = Camera { x: 10, y: 10 };
        let panned = cam.pan(-9999, -9999, &l, pane);
        assert_eq!(panned.x, 0);
        assert_eq!(panned.y, 0);
        let panned = cam.pan(9999, 9999, &l, pane);
        assert_eq!(panned.x, 500 - 80);
        assert_eq!(panned.y, 200 - 24);
    }

    #[test]
    fn off_screen_side_detects_each_side() {
        let l = layout_500x200();
        let pane = Rect::new(0, 0, 80, 24);
        let cam = Camera { x: 0, y: 0 };
        assert_eq!(cam.off_screen_side(&l.nodes[0], pane), Some(Side::Right));
        let cam = Camera { x: 200, y: 0 };
        assert_eq!(cam.off_screen_side(&l.nodes[0], pane), Some(Side::Left));
        let cam = Camera { x: 90, y: 40 };
        assert_eq!(cam.off_screen_side(&l.nodes[0], pane), None);
    }

    fn placed(id: &str, x: u16, y: u16, w: u16, h: u16) -> PlacedNode {
        PlacedNode {
            id: id.into(),
            kind: "unit".into(),
            label: id.into(),
            x,
            y,
            w,
            h,
            discovered: None,
        }
    }

    #[test]
    fn fit_bounds_centers_single_node_horizontally_and_aligns_top() {
        let l = layout_500x200();
        let pane = Rect::new(0, 0, 80, 24);
        let cam = Camera::fit_bounds(&l, pane);
        // Single node at (100, 50) size 22x3.
        // Horizontal: center of bbox is x=111; pane half is 40 → cam.x = 71.
        // Vertical: bbox top is y=50 → cam.y = 50 (no centering).
        assert_eq!(cam.x, 71);
        assert_eq!(cam.y, 50);
    }

    #[test]
    fn fit_bounds_centers_bounding_rect_horizontally_aligns_top_for_multiple_nodes() {
        let l = Layout {
            nodes: vec![
                placed("a", 100, 50, 22, 3),  // bbox top is y=50
                placed("b", 200, 80, 22, 3),  // bbox right edge is x=222
            ],
            edges: vec![] as Vec<EdgeRoute>,
            canvas_w: 500,
            canvas_h: 200,
        };
        let pane = Rect::new(0, 0, 80, 24);
        let cam = Camera::fit_bounds(&l, pane);
        // Horizontal: bbox spans x=100..222, center=161, pane half=40 → cam.x=121.
        // Vertical: bbox top is y=50.
        assert_eq!(cam.x, 121);
        assert_eq!(cam.y, 50);
    }

    #[test]
    fn fit_bounds_returns_origin_for_empty_layout() {
        let l = Layout {
            nodes: vec![],
            edges: vec![] as Vec<EdgeRoute>,
            canvas_w: 0,
            canvas_h: 0,
        };
        let pane = Rect::new(0, 0, 80, 24);
        let cam = Camera::fit_bounds(&l, pane);
        assert_eq!(cam, Camera::origin());
    }

    #[test]
    fn blit_copies_cells_from_canvas_to_dst() {
        let mut src = Buffer::empty(Rect::new(0, 0, 10, 10));
        src.cell_mut((5_u16, 5_u16)).unwrap().set_char('X');
        let mut dst = Buffer::empty(Rect::new(0, 0, 4, 4));
        blit(&src, Camera { x: 4, y: 4 }, Rect::new(0, 0, 4, 4), &mut dst);
        assert_eq!(dst.cell((1, 1)).unwrap().symbol(), "X");
    }
}
