//! The editor's canvas: the photo with the recipe's rotation and flips
//! applied live, and a crop box with handles.

use std::cell::{Cell, RefCell};

use gtk::{gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};

use crate::edit::{Recipe, Rect};

const MARGIN: f32 = 28.0;
const HANDLE: f64 = 22.0;
const MIN_SIZE: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Grab {
    Move,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CropCanvas {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub recipe: Cell<Recipe>,
        /// Width/height of the crop in pixels, when locked.
        pub aspect: Cell<Option<f64>>,
        pub grab: Cell<Option<Grab>>,
        pub start: Cell<Option<Rect>>,
        pub changed: RefCell<Option<Box<dyn Fn()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CropCanvas {
        const NAME: &'static str = "CloudPhotosCropCanvas";
        type Type = super::CropCanvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CropCanvas {
        fn constructed(&self) {
            self.parent_constructed();
            let widget = self.obj();
            widget.set_hexpand(true);
            widget.set_vexpand(true);
            widget.add_css_class("crop-canvas");

            let drag = gtk::GestureDrag::new();
            let w = widget.downgrade();
            drag.connect_drag_begin(move |_, x, y| {
                if let Some(w) = w.upgrade() {
                    w.begin(x, y);
                }
            });
            let w = widget.downgrade();
            drag.connect_drag_update(move |_, dx, dy| {
                if let Some(w) = w.upgrade() {
                    w.update(dx, dy);
                }
            });
            let w = widget.downgrade();
            drag.connect_drag_end(move |_, _, _| {
                if let Some(w) = w.upgrade() {
                    w.imp().grab.set(None);
                    w.queue_draw();
                }
            });
            widget.add_controller(drag);

            let motion = gtk::EventControllerMotion::new();
            let w = widget.downgrade();
            motion.connect_motion(move |_, x, y| {
                if let Some(w) = w.upgrade() {
                    let cursor = match w.hit(x, y) {
                        Some(Grab::Move) => "move",
                        Some(Grab::Left | Grab::Right) => "ew-resize",
                        Some(Grab::Top | Grab::Bottom) => "ns-resize",
                        Some(Grab::TopLeft | Grab::BottomRight) => "nwse-resize",
                        Some(Grab::TopRight | Grab::BottomLeft) => "nesw-resize",
                        None => "default",
                    };
                    w.set_cursor_from_name(Some(cursor));
                }
            });
            widget.add_controller(motion);
        }
    }

    impl WidgetImpl for CropCanvas {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct CropCanvas(ObjectSubclass<imp::CropCanvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for CropCanvas {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl CropCanvas {
    pub fn set_texture(&self, texture: Option<&gdk::Texture>) {
        self.imp().texture.replace(texture.cloned());
        self.queue_draw();
    }

    pub fn recipe(&self) -> Recipe {
        self.imp().recipe.get()
    }

    pub fn set_recipe(&self, recipe: Recipe) {
        self.imp().recipe.set(recipe);
        self.queue_draw();
        if let Some(f) = self.imp().changed.borrow().as_ref() {
            f();
        }
    }

    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        self.imp().changed.replace(Some(Box::new(f)));
    }

    /// Change the recipe with `f` (rotate, flip…), keeping the aspect lock.
    pub fn modify(&self, f: impl FnOnce(&mut Recipe)) {
        let mut recipe = self.recipe();
        f(&mut recipe);
        self.set_recipe(recipe);
        // A locked ratio is width/height of the picture as shown; after a
        // quarter turn the old crop is the transposed shape.
        if let Some(a) = self.imp().aspect.get() {
            self.set_aspect(Some(a));
        }
    }

    /// Size of the picture as shown (after rotation), in texture pixels.
    pub fn oriented_size(&self) -> (f64, f64) {
        let texture = self.imp().texture.borrow();
        let (w, h) = texture.as_ref().map(|t| (t.width() as u32, t.height() as u32)).unwrap_or((1, 1));
        let (w, h) = self.recipe().oriented_size(w, h);
        (w as f64, h as f64)
    }

    /// Lock the crop to `ratio` (width/height in pixels): the largest such
    /// box centered on the current crop. `None` unlocks.
    pub fn set_aspect(&self, ratio: Option<f64>) {
        self.imp().aspect.set(ratio);
        let Some(ratio) = ratio else { return };
        let (ow, oh) = self.oriented_size();
        let mut recipe = self.recipe();
        let c = recipe.crop;
        let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
        // In normalized units, width/height must equal ratio * oh / ow.
        let r = ratio * oh / ow;
        let (mut w, mut h) = (1.0, 1.0 / r);
        if h > 1.0 {
            h = 1.0;
            w = r;
        }
        let x = (cx - w / 2.0).clamp(0.0, 1.0 - w);
        let y = (cy - h / 2.0).clamp(0.0, 1.0 - h);
        recipe.crop = Rect { x, y, w, h };
        self.imp().recipe.set(recipe);
        self.queue_draw();
        if let Some(f) = self.imp().changed.borrow().as_ref() {
            f();
        }
    }

    /// Where the picture sits in the widget: (x, y, width, height).
    fn image_rect(&self) -> (f64, f64, f64, f64) {
        let (ow, oh) = self.oriented_size();
        let aw = (self.width() as f64 - 2.0 * MARGIN as f64).max(1.0);
        let ah = (self.height() as f64 - 2.0 * MARGIN as f64).max(1.0);
        let scale = (aw / ow).min(ah / oh);
        let (iw, ih) = (ow * scale, oh * scale);
        ((self.width() as f64 - iw) / 2.0, (self.height() as f64 - ih) / 2.0, iw, ih)
    }

    fn crop_rect(&self) -> (f64, f64, f64, f64) {
        let (ix, iy, iw, ih) = self.image_rect();
        let c = self.recipe().crop;
        (ix + c.x * iw, iy + c.y * ih, c.w * iw, c.h * ih)
    }

    fn hit(&self, x: f64, y: f64) -> Option<Grab> {
        let (cx, cy, cw, ch) = self.crop_rect();
        let near = |a: f64, b: f64| (a - b).abs() <= HANDLE;
        let (left, right, top, bottom) = (near(x, cx), near(x, cx + cw), near(y, cy), near(y, cy + ch));
        let inside_x = x >= cx - HANDLE && x <= cx + cw + HANDLE;
        let inside_y = y >= cy - HANDLE && y <= cy + ch + HANDLE;
        match (left, right, top, bottom) {
            (true, _, true, _) => Some(Grab::TopLeft),
            (_, true, true, _) => Some(Grab::TopRight),
            (true, _, _, true) => Some(Grab::BottomLeft),
            (_, true, _, true) => Some(Grab::BottomRight),
            (true, _, _, _) if inside_y => Some(Grab::Left),
            (_, true, _, _) if inside_y => Some(Grab::Right),
            (_, _, true, _) if inside_x => Some(Grab::Top),
            (_, _, _, true) if inside_x => Some(Grab::Bottom),
            _ if x > cx && x < cx + cw && y > cy && y < cy + ch => Some(Grab::Move),
            _ => None,
        }
    }

    fn begin(&self, x: f64, y: f64) {
        self.imp().grab.set(self.hit(x, y));
        self.imp().start.set(Some(self.recipe().crop));
    }

    fn update(&self, dx: f64, dy: f64) {
        let (Some(grab), Some(start)) = (self.imp().grab.get(), self.imp().start.get()) else { return };
        let (_, _, iw, ih) = self.image_rect();
        let (nx, ny) = (dx / iw, dy / ih);
        let mut c = start;
        if grab == Grab::Move {
            c.x = (start.x + nx).clamp(0.0, 1.0 - start.w);
            c.y = (start.y + ny).clamp(0.0, 1.0 - start.h);
        } else {
            let (mut l, mut t, mut r, mut b) = (start.x, start.y, start.x + start.w, start.y + start.h);
            if matches!(grab, Grab::Left | Grab::TopLeft | Grab::BottomLeft) {
                l = (l + nx).clamp(0.0, r - MIN_SIZE);
            }
            if matches!(grab, Grab::Right | Grab::TopRight | Grab::BottomRight) {
                r = (r + nx).clamp(l + MIN_SIZE, 1.0);
            }
            if matches!(grab, Grab::Top | Grab::TopLeft | Grab::TopRight) {
                t = (t + ny).clamp(0.0, b - MIN_SIZE);
            }
            if matches!(grab, Grab::Bottom | Grab::BottomLeft | Grab::BottomRight) {
                b = (b + ny).clamp(t + MIN_SIZE, 1.0);
            }
            c = Rect { x: l, y: t, w: r - l, h: b - t };
            if let Some(ratio) = self.imp().aspect.get() {
                c = constrain(c, grab, ratio, self.oriented_size());
            }
        }
        let mut recipe = self.recipe();
        recipe.crop = c;
        self.set_recipe(recipe);
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let Some(texture) = self.imp().texture.borrow().clone() else { return };
        let recipe = self.recipe();
        let (ix, iy, iw, ih) = self.image_rect();
        let (tw, th) = (texture.width() as f32, texture.height() as f32);
        let scale = if recipe.quarter_turns % 2 == 1 { ih as f32 / tw } else { iw as f32 / tw };

        // The picture: screen = translate · flip · rotate · texture.
        snapshot.save();
        snapshot.translate(&graphene::Point::new((ix + iw / 2.0) as f32, (iy + ih / 2.0) as f32));
        snapshot.scale(
            if recipe.flip_horizontal { -1.0 } else { 1.0 },
            if recipe.flip_vertical { -1.0 } else { 1.0 },
        );
        snapshot.rotate(90.0 * (recipe.quarter_turns % 4) as f32);
        let (dw, dh) = (tw * scale, th * scale);
        snapshot.append_scaled_texture(&texture, gsk::ScalingFilter::Trilinear, &graphene::Rect::new(-dw / 2.0, -dh / 2.0, dw, dh));
        snapshot.restore();

        // Dim outside the crop.
        let (cx, cy, cw, ch) = self.crop_rect();
        let shade = gdk::RGBA::new(0.0, 0.0, 0.0, 0.62);
        let r = |x: f64, y: f64, w: f64, h: f64| graphene::Rect::new(x as f32, y as f32, w.max(0.0) as f32, h.max(0.0) as f32);
        for rect in [
            r(ix, iy, iw, cy - iy),
            r(ix, cy + ch, iw, iy + ih - cy - ch),
            r(ix, cy, cx - ix, ch),
            r(cx + cw, cy, ix + iw - cx - cw, ch),
        ] {
            snapshot.append_color(&shade, &rect);
        }

        // Rule-of-thirds guides while dragging, the border, and corner handles.
        let white = gdk::RGBA::new(1.0, 1.0, 1.0, 1.0);
        if self.imp().grab.get().is_some() {
            let guide = gdk::RGBA::new(1.0, 1.0, 1.0, 0.45);
            for i in 1..3 {
                let f = i as f64 / 3.0;
                snapshot.append_color(&guide, &r(cx + cw * f, cy, 1.0, ch));
                snapshot.append_color(&guide, &r(cx, cy + ch * f, cw, 1.0));
            }
        }
        let outline = gsk::RoundedRect::from_rect(r(cx, cy, cw, ch), 0.0);
        snapshot.append_border(&outline, &[1.5; 4], &[white; 4]);
        let (len, thick) = (18.0, 4.0);
        for (x, y, sx, sy) in [(cx, cy, 1.0, 1.0), (cx + cw, cy, -1.0, 1.0), (cx, cy + ch, 1.0, -1.0), (cx + cw, cy + ch, -1.0, -1.0)] {
            let hx = if sx > 0.0 { x - thick / 2.0 } else { x - len + thick / 2.0 };
            let vy = if sy > 0.0 { y - thick / 2.0 } else { y - len + thick / 2.0 };
            snapshot.append_color(&white, &r(hx, y - thick / 2.0, len, thick));
            snapshot.append_color(&white, &r(x - thick / 2.0, vy, thick, len));
        }
    }
}

/// Keep `c` at `ratio` (pixel width/height) after dragging `grab`, anchored
/// at the opposite corner or edge, and inside the picture.
fn constrain(c: Rect, grab: Grab, ratio: f64, (ow, oh): (f64, f64)) -> Rect {
    let r = ratio * oh / ow; // normalized width/height
    let (mut w, mut h) = (c.w, c.h);
    match grab {
        Grab::Top | Grab::Bottom => w = h * r,
        _ => h = w / r,
    }
    // Shrink to fit inside 0..1 from the anchor.
    let anchor_right = matches!(grab, Grab::Left | Grab::TopLeft | Grab::BottomLeft);
    let anchor_bottom = matches!(grab, Grab::Top | Grab::TopLeft | Grab::TopRight);
    let (right, bottom) = (c.x + c.w, c.y + c.h);
    let max_w = if anchor_right { right } else if matches!(grab, Grab::Top | Grab::Bottom) { 1.0 } else { 1.0 - c.x };
    let max_h = if anchor_bottom { bottom } else if matches!(grab, Grab::Left | Grab::Right) { 1.0 } else { 1.0 - c.y };
    let fit = (max_w / w).min(max_h / h).min(1.0);
    w *= fit;
    h *= fit;
    let x = match grab {
        Grab::Top | Grab::Bottom => (c.x + c.w / 2.0 - w / 2.0).clamp(0.0, 1.0 - w),
        _ if anchor_right => right - w,
        _ => c.x,
    };
    let y = match grab {
        Grab::Left | Grab::Right => (c.y + c.h / 2.0 - h / 2.0).clamp(0.0, 1.0 - h),
        _ if anchor_bottom => bottom - h,
        _ => c.y,
    };
    Rect { x, y, w, h }
}
