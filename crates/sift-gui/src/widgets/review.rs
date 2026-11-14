//! The review list: grouped rows with checkboxes and sizes. Nothing is
//! removed from here — it only records what you ticked.

use super::{fmt_bytes, hbox, label, vbox};
use gtk::pango;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

#[derive(Clone, Debug, Default)]
pub struct ReviewRow {
    pub id: String,
    pub group: String,
    /// Order key for groups (lower first).
    pub group_order: u32,
    pub title: String,
    pub subtitle: String,
    pub bytes: u64,
    pub approx: bool,
    pub selected: bool,
    pub blocked: Option<String>,
    pub note: String,
    pub badge: Option<(String, String)>,
}

pub struct ReviewList {
    pub widget: gtk::ListBox,
    rows: RefCell<Vec<ReviewRow>>,
    selected: RefCell<HashSet<String>>,
    checks: RefCell<HashMap<String, gtk::CheckButton>>,
    on_change: RefCell<Option<Rc<dyn Fn()>>>,
}

impl ReviewList {
    pub fn new() -> Rc<ReviewList> {
        let list = gtk::ListBox::new();
        list.add_css_class("list");
        list.set_selection_mode(gtk::SelectionMode::None);
        // Clicking anywhere on a row toggles its checkbox.
        list.connect_row_activated(|_, row| {
            if let Some(cb) = row.child().and_then(|c| c.first_child()).and_downcast::<gtk::CheckButton>() {
                if cb.is_sensitive() {
                    cb.set_active(!cb.is_active());
                }
            }
        });
        Rc::new(ReviewList {
            widget: list,
            rows: RefCell::new(Vec::new()),
            selected: RefCell::new(HashSet::new()),
            checks: RefCell::new(HashMap::new()),
            on_change: RefCell::new(None),
        })
    }

    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Rc::new(f));
    }

    fn changed(&self) {
        let f = self.on_change.borrow().clone();
        if let Some(f) = f {
            f();
        }
    }

    pub fn selected_ids(&self) -> Vec<String> {
        let sel = self.selected.borrow();
        self.rows.borrow().iter().filter(|r| sel.contains(&r.id)).map(|r| r.id.clone()).collect()
    }

    /// (bytes, count) of the current selection (approximate rows excluded from bytes).
    pub fn selection_size(&self) -> (u64, usize) {
        let sel = self.selected.borrow();
        let rows = self.rows.borrow();
        let chosen: Vec<&ReviewRow> = rows.iter().filter(|r| sel.contains(&r.id)).collect();
        (chosen.iter().filter(|r| !r.approx).map(|r| r.bytes).sum(), chosen.len())
    }

    pub fn set_selected(&self, ids: &HashSet<String>) {
        let checks = self.checks.borrow().clone();
        for (id, cb) in checks {
            cb.set_active(ids.contains(&id));
        }
    }

    pub fn set_rows(self: &Rc<Self>, rows: Vec<ReviewRow>) {
        super::clear_list(&self.widget);
        self.checks.borrow_mut().clear();
        *self.selected.borrow_mut() = rows.iter().filter(|r| r.selected && r.blocked.is_none()).map(|r| r.id.clone()).collect();
        let mut groups: BTreeMap<(u32, String), Vec<ReviewRow>> = BTreeMap::new();
        for r in &rows {
            groups.entry((r.group_order, r.group.clone())).or_default().push(r.clone());
        }
        *self.rows.borrow_mut() = rows;

        for ((_, group), items) in groups {
            let total: u64 = items.iter().filter(|r| !r.approx).map(|r| r.bytes).sum();
            let head = hbox(10);
            head.add_css_class("group-header");
            let gcheck = gtk::CheckButton::new();
            let selectable: Vec<String> = items.iter().filter(|r| r.blocked.is_none()).map(|r| r.id.clone()).collect();
            gcheck.set_active(!selectable.is_empty() && selectable.iter().all(|id| self.selected.borrow().contains(id)));
            gcheck.set_sensitive(!selectable.is_empty());
            head.append(&gcheck);
            let gl = label(&group, &["section-title"]);
            gl.set_hexpand(true);
            head.append(&gl);
            head.append(&label(&fmt_bytes(total), &["mono", "dim"]));
            let row = gtk::ListBoxRow::new();
            row.set_activatable(false);
            row.set_child(Some(&head));
            self.widget.append(&row);

            let mut group_checks = Vec::new();
            for item in items {
                let line = hbox(12);
                line.add_css_class("row-box");
                let cb = gtk::CheckButton::new();
                cb.set_active(self.selected.borrow().contains(&item.id));
                cb.set_sensitive(item.blocked.is_none());
                cb.set_valign(gtk::Align::Center);
                line.append(&cb);
                let text = vbox(2);
                text.set_hexpand(true);
                let title = label(&item.title, &["bold"]);
                title.set_ellipsize(pango::EllipsizeMode::End);
                text.append(&title);
                if !item.subtitle.is_empty() {
                    let sub = label(&item.subtitle, &["mono", "tiny", "dim"]);
                    sub.set_ellipsize(pango::EllipsizeMode::Middle);
                    sub.set_tooltip_text(Some(&item.subtitle));
                    text.append(&sub);
                }
                if !item.note.is_empty() {
                    let n = label(&item.note, &["small", "faint"]);
                    n.set_wrap(true);
                    text.append(&n);
                }
                line.append(&text);
                if let Some(b) = &item.blocked {
                    line.append(&super::chip(b, "warn"));
                }
                if let Some((t, k)) = &item.badge {
                    line.append(&super::chip(t, k));
                }
                let size = if item.approx { format!("≤ {}", fmt_bytes(item.bytes)) } else { fmt_bytes(item.bytes) };
                let sl = label(&size, &["mono"]);
                sl.set_width_chars(10);
                sl.set_xalign(1.0);
                line.append(&sl);
                let r = gtk::ListBoxRow::new();
                r.set_activatable(item.blocked.is_none());
                r.set_child(Some(&line));
                self.widget.append(&r);

                let me = Rc::downgrade(self);
                let id = item.id.clone();
                cb.connect_toggled(move |c| {
                    if let Some(me) = me.upgrade() {
                        if c.is_active() {
                            me.selected.borrow_mut().insert(id.clone());
                        } else {
                            me.selected.borrow_mut().remove(&id);
                        }
                        me.changed();
                    }
                });
                self.checks.borrow_mut().insert(item.id.clone(), cb.clone());
                if item.blocked.is_none() {
                    group_checks.push(cb);
                }
            }
            gcheck.connect_toggled(move |g| {
                for c in &group_checks {
                    c.set_active(g.is_active());
                }
            });
        }
        self.changed();
    }
}
