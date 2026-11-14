//! Sift's look: Burrow's warm espresso / cream palette, per-tool accents,
//! Geist + Cal Sans type. Regenerated whenever light/dark changes.

use gtk::gdk;
use std::cell::Cell;

#[derive(Clone, Copy, Debug)]
pub struct Rgb(pub u32);

impl Rgb {
    pub fn rgba(self, a: f64) -> (f64, f64, f64, f64) {
        let r = ((self.0 >> 16) & 0xff) as f64 / 255.0;
        let g = ((self.0 >> 8) & 0xff) as f64 / 255.0;
        let b = (self.0 & 0xff) as f64 / 255.0;
        (r, g, b, a)
    }

    pub fn css(self) -> String {
        format!("#{:06X}", self.0)
    }

    pub fn css_a(self, a: f64) -> String {
        let (r, g, b, _) = self.rgba(1.0);
        format!("rgba({},{},{},{a:.3})", (r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    }
}

pub const TEAL: Rgb = Rgb(0x35C2A5);
pub const MOSS: Rgb = Rgb(0x6FB06A);
pub const GINGER: Rgb = Rgb(0xD98C5F);
pub const CORAL: Rgb = Rgb(0xF0714E);
pub const VIOLET: Rgb = Rgb(0x8E84F0);
pub const AZURE: Rgb = Rgb(0x4FA3E3);
pub const ROSE: Rgb = Rgb(0xDB7E9C);
pub const LILAC: Rgb = Rgb(0xB58BD6);
pub const GOLD: Rgb = Rgb(0xE6A93C);
pub const GREEN: Rgb = Rgb(0x3CB371);
pub const RED: Rgb = Rgb(0xF0604E);
pub const ORANGE: Rgb = Rgb(0xF2894E);
pub const BLUE: Rgb = Rgb(0x5AA8F0);
pub const INDIGO: Rgb = Rgb(0x6E8FE0);
pub const SAGE: Rgb = Rgb(0x5AA98B);

/// Tools and their accents (rail order).
pub const TOOLS: &[(&str, &str, Rgb)] = &[
    ("clean", "Clean", TEAL),
    ("optimize", "Optimize", VIOLET),
    ("apps", "Apps", CORAL),
    ("analyze", "Analyze", AZURE),
    ("dupes", "Duplicates", ROSE),
    ("ports", "Ports", LILAC),
];

pub fn accent_for(page: &str) -> Rgb {
    match page {
        "monitor" => GOLD,
        "settings" => SAGE,
        "purge" => MOSS,
        "installer" => GINGER,
        _ => TOOLS.iter().find(|t| t.0 == page).map(|t| t.2).unwrap_or(GOLD),
    }
}

thread_local! {
    static DARK: Cell<bool> = const { Cell::new(true) };
}

pub fn is_dark() -> bool {
    DARK.with(|d| d.get())
}

/// Palette for drawing code.
#[derive(Clone, Copy)]
pub struct Palette {
    pub base: Rgb,
    pub ink: Rgb,
    pub dark: bool,
}

impl Palette {
    pub fn ink(&self, a: f64) -> (f64, f64, f64, f64) {
        self.ink.rgba(a)
    }
    pub fn track(&self) -> (f64, f64, f64, f64) {
        self.ink.rgba(if self.dark { 0.10 } else { 0.10 })
    }
}

pub fn palette() -> Palette {
    if is_dark() {
        Palette { base: Rgb(0x17120A), ink: Rgb(0xEDE6DA), dark: true }
    } else {
        Palette { base: Rgb(0xF4EFE6), ink: Rgb(0x221B11), dark: false }
    }
}

pub fn set_source(cr: &gtk::cairo::Context, c: (f64, f64, f64, f64)) {
    cr.set_source_rgba(c.0, c.1, c.2, c.3);
}

pub fn css(dark: bool) -> String {
    let (base, soft, ink, ink2) = if dark {
        (Rgb(0x17120A), Rgb(0x211A11), Rgb(0xEDE6DA), Rgb(0xF2ECE0))
    } else {
        (Rgb(0xF4EFE6), Rgb(0xFCF9F3), Rgb(0x221B11), Rgb(0x2A2114))
    };
    let a = |d: f64, l: f64| if dark { d } else { l };
    let card = if dark { ink2.css_a(0.065) } else { "rgba(255,255,255,0.70)".to_string() };
    let card_hover = if dark { ink2.css_a(0.10) } else { "rgba(255,255,255,0.92)".to_string() };
    let mut s = format!(
        r#"
@define-color window_bg_color {base};
@define-color window_fg_color {ink};
@define-color view_bg_color {soft};
@define-color view_fg_color {ink};
@define-color headerbar_bg_color {base};
@define-color headerbar_fg_color {ink};
@define-color card_bg_color {card};
@define-color dialog_bg_color {soft};
@define-color popover_bg_color {soft};
@define-color accent_bg_color #D9A066;
@define-color accent_fg_color #241B12;
@define-color accent_color #D9A066;

window.sift-window {{
  background-color: {base};
  background-image: radial-gradient(circle at 0% 0%, {glow}, transparent 55%);
  color: {ink};
  font-family: "Geist", "Inter", "Cantarell", sans-serif;
}}
.sift-window headerbar {{ background: transparent; box-shadow: none; min-height: 38px; }}
.mono {{ font-family: "Geist Mono", "JetBrains Mono", "Fira Mono", monospace; }}
.dim {{ color: {dim}; }}
.faint {{ color: {faint}; }}
.small {{ font-size: 0.86em; }}
.tiny {{ font-size: 0.78em; }}
.bold {{ font-weight: 600; }}

.rail {{
  background-color: {rail};
  border: 1px solid {hair};
  border-radius: 26px;
  padding: 12px 8px;
  margin: 6px 0 14px 14px;
}}
.rail-btn {{
  min-width: 46px; min-height: 46px; padding: 0;
  border-radius: 15px; background: transparent; color: {dim}; box-shadow: none;
}}
.rail-btn:hover {{ background-color: {chip}; color: {ink}; }}
.rail-logo {{ min-width: 52px; min-height: 52px; border-radius: 18px; background-color: {chip}; }}
.rail-sep {{ background-color: {hair}; min-height: 1px; margin: 6px 10px; }}

.page {{ padding: 4px 26px 20px 22px; }}
.page-title {{ font-family: "Cal Sans", "Geist", sans-serif; font-size: 28px; }}
.tagline {{ font-style: italic; color: {dim}; font-size: 15px; }}
.section-title {{ font-family: "Cal Sans", "Geist", sans-serif; font-size: 18px; }}

.seg {{ background-color: {chip}; border-radius: 999px; padding: 4px; border: 1px solid {hair}; }}
.seg-btn {{
  border-radius: 999px; padding: 6px 16px; background: transparent; box-shadow: none; color: {dim};
  font-family: "Geist Mono", monospace; font-size: 14px;
}}
.seg-btn:checked {{ background-color: {ink}; color: {base}; }}
.seg-btn:hover:not(:checked) {{ color: {ink}; }}

.card {{
  background-color: {card};
  border: 1px solid {hair};
  border-radius: 18px;
  padding: 18px 20px;
}}
.card.hoverable:hover {{ background-color: {card_hover}; }}
.card-title {{ font-family: "Geist Mono", monospace; font-weight: 600; letter-spacing: 2px; font-size: 13px; }}
.big-number {{ font-family: "Geist Mono", monospace; font-weight: 700; font-size: 40px; }}
.huge-number {{ font-family: "Cal Sans", "Geist", sans-serif; font-size: 64px; }}
.unit {{ font-family: "Geist Mono", monospace; font-size: 16px; color: {dim}; }}
.orb-title {{ font-family: "Geist Mono", monospace; font-weight: 700; font-size: 46px; }}

.chip {{
  background-color: {chip}; border-radius: 999px; padding: 2px 10px;
  font-family: "Geist Mono", monospace; font-size: 12px; color: {dim};
}}
.chip.good {{ color: {green}; background-color: {green_a}; }}
.chip.warn {{ color: {gold}; background-color: {gold_a}; }}
.chip.bad {{ color: {red}; background-color: {red_a}; }}
.chip.info {{ color: {blue}; background-color: {blue_a}; }}

button.pill {{ border-radius: 999px; padding: 8px 22px; font-weight: 600; }}
button.primary {{ background-color: {ink}; color: {base}; }}
button.primary:hover {{ background-color: {ink_hover}; }}
button.primary:disabled {{ background-color: {chip}; color: {faint}; }}
button.ghost {{ background-color: {chip}; color: {ink}; }}
button.flat-link {{ background: transparent; color: {dim}; box-shadow: none; }}
button.flat-link:hover {{ color: {ink}; }}
button.danger {{ background-color: {red_a}; color: {red}; }}
button.small-btn {{ padding: 2px 12px; min-height: 26px; border-radius: 999px; font-size: 13px; }}

.list {{ background: transparent; }}
.list > row {{ background: transparent; border-radius: 12px; padding: 0; }}
.list > row:hover {{ background-color: {chip}; }}
.list > row:selected {{ background-color: {chip}; color: {ink}; }}
.row-box {{ padding: 8px 10px; }}
.group-header {{ padding: 14px 10px 4px 10px; }}
.hair {{ background-color: {hair}; min-height: 1px; }}
.table-head {{ font-family: "Geist Mono", monospace; font-size: 12px; color: {faint}; letter-spacing: 1px; }}

.bottom-bar {{ border-top: 1px solid {hair}; padding: 12px 4px 0 4px; }}
.crumb {{ font-family: "Geist Mono", monospace; color: {dim}; }}
.crumb-btn {{ background: transparent; box-shadow: none; padding: 2px 6px; font-family: "Geist Mono", monospace; color: {dim}; }}
.crumb-btn:hover {{ color: {ink}; }}
.hover-info {{ font-family: "Geist Mono", monospace; font-size: 13px; color: {dim}; }}
progressbar > trough {{ background-color: {chip}; border-radius: 99px; min-height: 8px; }}
progressbar > trough > progress {{ border-radius: 99px; min-height: 8px; }}
entry, searchbar entry {{ border-radius: 12px; }}
.empty-title {{ font-family: "Cal Sans", "Geist", sans-serif; font-size: 22px; }}
"#,
        base = base.css(),
        soft = soft.css(),
        ink = ink.css(),
        card = card,
        card_hover = card_hover,
        glow = Rgb(0xD9A066).css_a(a(0.07, 0.10)),
        dim = ink.css_a(0.62),
        faint = ink.css_a(a(0.40, 0.46)),
        rail = ink2.css_a(a(0.045, 0.0)),
        hair = ink2.css_a(a(0.10, 0.12)),
        chip = ink2.css_a(a(0.08, 0.07)),
        ink_hover = ink.css_a(0.86),
        green = GREEN.css(),
        green_a = GREEN.css_a(0.14),
        gold = GOLD.css(),
        gold_a = GOLD.css_a(0.14),
        red = RED.css(),
        red_a = RED.css_a(0.14),
        blue = BLUE.css(),
        blue_a = BLUE.css_a(0.14),
    );
    // Per-tool accents: active rail button, accent text, accent buttons.
    let mut pages: Vec<(&str, Rgb)> = TOOLS.iter().map(|t| (t.0, t.2)).collect();
    pages.extend([("monitor", GOLD), ("settings", SAGE), ("purge", MOSS), ("installer", GINGER)]);
    for (name, c) in pages {
        s.push_str(&format!(
            ".rail-btn.active.accent-{name} {{ background-color: {c}; color: #08101C; }}\n\
             .accent-{name} .accent, .accent-{name}.accent {{ color: {c}; }}\n\
             .accent-{name} button.accent-btn {{ background-color: {c}; color: #08101C; }}\n\
             .accent-{name} button.accent-btn:disabled {{ background-color: {chip}; color: {faint}; }}\n\
             .accent-{name} progressbar > trough > progress {{ background-color: {c}; }}\n\
             .accent-{name} checkbutton check:checked {{ background-color: {c}; color: #08101C; }}\n",
            c = c.css(),
            chip = ink2.css_a(a(0.08, 0.07)),
            faint = ink.css_a(a(0.40, 0.46)),
        ));
    }
    s
}

thread_local! {
    static PROVIDER: gtk::CssProvider = gtk::CssProvider::new();
    static ADDED: Cell<bool> = const { Cell::new(false) };
}

/// Install (or refresh) the app stylesheet for the current light/dark mode.
pub fn apply(dark: bool) {
    DARK.with(|d| d.set(dark));
    PROVIDER.with(|p| {
        p.load_from_string(&css(dark));
        if !ADDED.with(|a| a.get()) {
            if let Some(display) = gdk::Display::default() {
                gtk::style_context_add_provider_for_display(&display, p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
                ADDED.with(|a| a.set(true));
            }
        }
    });
}


pub fn health_color(score: u8) -> Rgb {
    match score {
        90..=100 => GREEN,
        75..=89 => TEAL,
        60..=74 => GOLD,
        40..=59 => ORANGE,
        _ => RED,
    }
}
