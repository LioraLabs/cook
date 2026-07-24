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
fn pan_clamps_to_bbox_keeping_at_least_one_cell_visible() {
    let l = layout_500x200();
    let pane = Rect::new(0, 0, 80, 24);
    let cam = Camera { x: 50, y: 40 };
    // Single node at (100, 50, 22, 3). bbox = (100..122, 50..53).
    // cam_min = (100 - 80 + 1, 50 - 24 + 1) = (21, 27).
    // cam_max = (122 - 1, 53 - 1) = (121, 52).
    let panned = cam.pan(-9999, -9999, &l, pane);
    assert_eq!(panned.x, 21);
    assert_eq!(panned.y, 27);
    let panned = cam.pan(9999, 9999, &l, pane);
    assert_eq!(panned.x, 121);
    assert_eq!(panned.y, 52);
}

#[test]
fn pan_does_not_snap_a_centered_camera_back_to_canvas_origin() {
    // Regression: when fit_bounds returns a camera position outside
    // the old canvas-based [0, canvas-pane] range (e.g. negative x
    // for a horizontally-centered narrow bbox), pan must not yank
    // the camera back to that range on the first key press.
    let l = Layout {
        nodes: vec![placed("a", 10, 10, 22, 3)],
        edges: vec![] as Vec<EdgeRoute>,
        canvas_w: 40,
        canvas_h: 20,
    };
    let pane = Rect::new(0, 0, 80, 24);
    // fit_bounds: bbox center x = 21, pane half = 40, cam.x = -19.
    let cam = Camera::fit_bounds(&l, pane);
    assert_eq!(cam.x, -19);
    // Pan by zero: camera stays put.
    let same = cam.pan(0, 0, &l, pane);
    assert_eq!(same, cam);
    // Small pan right: camera moves; does not snap to 0.
    let nudged = cam.pan(5, 0, &l, pane);
    assert_eq!(nudged.x, -14);
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
