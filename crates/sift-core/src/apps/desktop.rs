//! freedesktop.org desktop entries (`*.desktop`).

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopEntry {
    pub file: PathBuf,
    pub id: String,
    pub name: String,
    pub exec: Option<String>,
    pub icon: Option<String>,
    pub startup_wm_class: Option<String>,
    pub flatpak_id: Option<String>,
    pub snap_name: Option<String>,
    pub no_display: bool,
    pub hidden: bool,
    pub is_app: bool,
}

pub fn parse(file: &Path, text: &str) -> DesktopEntry {
    let mut e = DesktopEntry {
        file: file.to_path_buf(),
        id: file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        ..Default::default()
    };
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim().to_string());
        match k {
            "Name" => e.name = v,
            "Exec" => e.exec = Some(v),
            "Icon" => e.icon = Some(v),
            "StartupWMClass" => e.startup_wm_class = Some(v),
            "X-Flatpak" => e.flatpak_id = Some(v),
            "X-SnapInstanceName" => e.snap_name = Some(v),
            "NoDisplay" => e.no_display = v.eq_ignore_ascii_case("true"),
            "Hidden" => e.hidden = v.eq_ignore_ascii_case("true"),
            "Type" => e.is_app = v == "Application",
            _ => {}
        }
    }
    e
}

pub fn read_dir(dir: &Path) -> Vec<DesktopEntry> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for f in rd.flatten() {
        let p = f.path();
        if p.extension().map(|x| x == "desktop").unwrap_or(false) {
            if let Ok(text) = fs::read_to_string(&p) {
                let e = parse(&p, &text);
                if e.is_app && !e.no_display && !e.hidden && !e.name.is_empty() {
                    out.push(e);
                }
            }
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_main_section_only() {
        let t = "[Desktop Entry]\nType=Application\nName=Obsidian\nName[de]=Obsidian DE\nExec=obsidian %U\nIcon=md.obsidian.Obsidian\nStartupWMClass=obsidian\nX-Flatpak=md.obsidian.Obsidian\n\n[Desktop Action new]\nName=New window\nExec=other\n";
        let e = parse(Path::new("/x/md.obsidian.Obsidian.desktop"), t);
        assert_eq!(e.name, "Obsidian");
        assert_eq!(e.exec.as_deref(), Some("obsidian %U"));
        assert_eq!(e.flatpak_id.as_deref(), Some("md.obsidian.Obsidian"));
        assert_eq!(e.id, "md.obsidian.Obsidian");
        assert!(e.is_app);
    }
}
