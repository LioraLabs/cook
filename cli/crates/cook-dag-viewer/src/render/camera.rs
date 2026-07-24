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
        let Some(first) = layout.nodes.first() else {
            return *self;
        };
        let mut min_x = first.x as i32;
        let mut min_y = first.y as i32;
        let mut max_x = first.x as i32 + first.w as i32;
        let mut max_y = first.y as i32 + first.h as i32;
        for n in &layout.nodes[1..] {
            min_x = min_x.min(n.x as i32);
            min_y = min_y.min(n.y as i32);
            max_x = max_x.max(n.x as i32 + n.w as i32);
            max_y = max_y.max(n.y as i32 + n.h as i32);
        }
        // Camera bounds keep at least one row/column of the bbox in view.
        // Range is bbox-derived, not canvas-derived, so the camera can sit
        // wherever fit_bounds places it (which may be negative for small
        // bboxes the renderer centers horizontally).
        let cam_min_x = min_x - pane.width as i32 + 1;
        let cam_max_x = max_x - 1;
        let cam_min_y = min_y - pane.height as i32 + 1;
        let cam_max_y = max_y - 1;
        Self {
            x: (self.x + dx).clamp(cam_min_x, cam_max_x),
            y: (self.y + dy).clamp(cam_min_y, cam_max_y),
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
#[path = "tests/camera_tests.rs"]
mod tests;
