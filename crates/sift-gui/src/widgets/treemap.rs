//! Interactive squarified treemap of one level of an analyze tree.

use crate::theme::{self, Rgb};
use gtk::pango;
use gtk::prelude::*;
use sift_core::analyze::Tree;
use sift_core::treemap::{squarify, Rect};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const COLORS: &[Rgb] = &[
    theme::AZURE, theme::TEAL, theme::GOLD, theme::CORAL, theme::VIOLET, theme::MOSS, theme::ROSE, theme::GINGER,
    theme::LILAC, theme::INDIGO, theme::SAGE,
];

pub struct Treemap {
    pub area: gtk::DrawingArea,
    tree: RefCell<Option<Rc<Tree>>>,
    node: Cell<usize>,
    layout: RefCell<Vec<(usize, Rect)>>,
    hover: Cell<Option<usize>>,
    on_open: RefCell<Option<Box<dyn Fn(usize)>>>,
    on_hover: RefCell<Option<Box<dyn Fn(Option<usize>)>>>,
}

impl Treemap {
    pub fn new() -> Rc<Treemap> {
        let area = gtk::DrawingArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_content_height(360);
        let t = Rc::new(Treemap {
            area,
            tree: RefCell::new(None),
            node: Cell::new(0),
            layout: RefCell::new(Vec::new()),
            hover: Cell::new(None),
            on_open: RefCell::new(None),
            on_hover: RefCell::new(None),
        });
        let weak = Rc::downgrade(&t);
        t.area.set_draw_func(move |_, cr, w, h| {
            if let Some(t) = weak.upgrade() {
                t.draw(cr, w as f64, h as f64);
            }
        });
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(&t);
        click.connect_released(move |_, _, x, y| {
            if let Some(t) = weak.upgrade() {
                if let Some(idx) = t.hit(x, y) {
                    if let Some(f) = t.on_open.borrow().as_ref() {
                        f(idx);
                    }
                }
            }
        });
        t.area.add_controller(click);
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&t);
        motion.connect_motion(move |_, x, y| {
            if let Some(t) = weak.upgrade() {
                let h = t.hit(x, y);
                if h != t.hover.get() {
                    t.hover.set(h);
                    t.area.queue_draw();
                    if let Some(f) = t.on_hover.borrow().as_ref() {
                        f(h);
                    }
                }
            }
        });
        let weak = Rc::downgrade(&t);
        motion.connect_leave(move |_| {
            if let Some(t) = weak.upgrade() {
                t.hover.set(None);
                t.area.queue_draw();
                if let Some(f) = t.on_hover.borrow().as_ref() {
                    f(None);
                }
            }
        });
        t.area.add_controller(motion);
        t
    }

    pub fn connect_open(&self, f: impl Fn(usize) + 'static) {
        *self.on_open.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_hover(&self, f: impl Fn(Option<usize>) + 'static) {
        *self.on_hover.borrow_mut() = Some(Box::new(f));
    }

    pub fn show(&self, tree: Rc<Tree>, node: usize) {
        *self.tree.borrow_mut() = Some(tree);
        self.node.set(node);
        self.hover.set(None);
        self.area.queue_draw();
    }


    fn hit(&self, x: f64, y: f64) -> Option<usize> {
        self.layout.borrow().iter().find(|(_, r)| r.contains(x, y)).map(|(i, _)| *i)
    }

    fn draw(&self, cr: &gtk::cairo::Context, w: f64, h: f64) {
        let pal = theme::palette();
        let tree = self.tree.borrow();
        let Some(tree) = tree.as_ref() else {
            self.layout.borrow_mut().clear();
            return;
        };
        let kids: Vec<usize> = tree.sorted_children(self.node.get()).into_iter().filter(|&k| tree.nodes[k].bytes > 0).take(400).collect();
        let sizes: Vec<f64> = kids.iter().map(|&k| tree.nodes[k].bytes as f64).collect();
        let rects = squarify(&sizes, Rect { x: 0.0, y: 0.0, w, h });
        let mut layout = Vec::with_capacity(kids.len());
        let font = pango::FontDescription::from_string("Geist Mono 9");
        let hover = self.hover.get();
        for (i, (&k, r)) in kids.iter().zip(rects.iter()).enumerate() {
            if r.w < 1.0 || r.h < 1.0 {
                continue;
            }
            let n = &tree.nodes[k];
            let color = if n.is_rest { Rgb(0x8A8070) } else { COLORS[i % COLORS.len()] };
            let inset = 1.5;
            let (x, y, rw, rh) = (r.x + inset, r.y + inset, (r.w - inset * 2.0).max(0.5), (r.h - inset * 2.0).max(0.5));
            let alpha = if Some(k) == hover { 0.95 } else if n.is_dir { 0.72 } else { 0.42 };
            let (cr_, cg, cb, _) = color.rgba(1.0);
            cr.set_source_rgba(cr_, cg, cb, alpha);
            super::charts::rounded(cr, x, y, rw, rh, 6.0_f64.min(rw / 3.0).min(rh / 3.0));
            let _ = cr.fill();
            if rw > 56.0 && rh > 30.0 {
                let layout_t = pangocairo::functions::create_layout(cr);
                layout_t.set_font_description(Some(&font));
                layout_t.set_width(((rw - 12.0) * pango::SCALE as f64) as i32);
                layout_t.set_ellipsize(pango::EllipsizeMode::End);
                let size = sift_core::format::bytes(n.bytes);
                layout_t.set_text(&format!("{}{}\n{}", n.name, if n.is_dir { "/" } else { "" }, size));
                cr.move_to(x + 6.0, y + 5.0);
                let ink = if pal.dark { (1.0, 0.98, 0.94, 0.95) } else { (0.08, 0.06, 0.03, 0.9) };
                cr.set_source_rgba(ink.0, ink.1, ink.2, ink.3);
                pangocairo::functions::show_layout(cr, &layout_t);
            }
            layout.push((k, *r));
        }
        *self.layout.borrow_mut() = layout;
    }
}
