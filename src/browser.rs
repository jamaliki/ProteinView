use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub const PANEL_WIDTH: u16 = 34;
const MIN_VIEWPORT_WIDTH: u16 = 20;

#[derive(Debug, Clone)]
pub struct BrowserEntry {
    pub path: PathBuf,
    pub label: String,
}

#[derive(Debug)]
pub struct FileBrowser {
    pub root: PathBuf,
    pub entries: Vec<BrowserEntry>,
    pub selected: usize,
    pub current: PathBuf,
    pub visible: bool,
    pub focused: bool,
    pub error: Option<String>,
}

impl FileBrowser {
    pub fn open_directory(root: &Path) -> Result<Self> {
        let root = fs::canonicalize(root)
            .with_context(|| format!("cannot open directory '{}'", root.display()))?;
        let entries = discover(&root)?;
        let Some(first) = entries.first() else {
            bail!(
                "no PDB, CIF, mmCIF, ENT, or XYZ files found in '{}'",
                root.display()
            );
        };
        let current = first.path.clone();
        Ok(Self {
            root,
            entries,
            selected: 0,
            current,
            visible: true,
            focused: true,
            error: None,
        })
    }

    pub fn alongside_file(file: &Path) -> Result<Self> {
        let file = fs::canonicalize(file)
            .with_context(|| format!("cannot open structure '{}'", file.display()))?;
        let root = file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let entries = discover(&root)?;
        let selected = entries
            .iter()
            .position(|entry| entry.path == file)
            .unwrap_or(0);
        Ok(Self {
            root,
            entries,
            selected,
            current: file,
            visible: false,
            focused: false,
            error: None,
        })
    }

    pub fn selected_path(&self) -> &Path {
        &self.entries[self.selected].path
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.entries.len().saturating_sub(1));
        self.error = None;
    }

    pub fn page(&mut self, delta: isize, height: usize) {
        self.move_selection(delta.saturating_mul(height.max(1) as isize));
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.error = None;
    }

    pub fn select_last(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
        self.error = None;
    }

    pub fn mark_loaded(&mut self, path: &Path) {
        self.current = path.to_path_buf();
        self.error = None;
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        self.focused = self.visible;
    }

    pub fn toggle_focus(&mut self) {
        if self.visible {
            self.focused = !self.focused;
        }
    }
}

pub fn panel_width(term_width: u16, visible: bool) -> u16 {
    if visible {
        PANEL_WIDTH.min(term_width.saturating_sub(MIN_VIEWPORT_WIDTH))
    } else {
        0
    }
}

fn discover(root: &Path) -> Result<Vec<BrowserEntry>> {
    let mut paths = Vec::new();
    collect_structure_files(root, &mut paths)?;
    paths.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    Ok(paths
        .into_iter()
        .map(|path| BrowserEntry {
            label: path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned(),
            path,
        })
        .collect())
}

fn collect_structure_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("cannot read directory '{}'", directory.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());

    for entry in entries {
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            collect_structure_files(&path, files)?;
        } else if file_type.is_file() && is_structure(&path) {
            files.push(path);
        }
    }
    Ok(())
}

fn is_structure(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "pdb" | "ent" | "cif" | "mmcif" | "xyz"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_supported_files_recursively_and_sorts_them() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::create_dir(temp.path().join(".hidden")).unwrap();
        fs::write(temp.path().join("z.PDB"), "").unwrap();
        fs::write(temp.path().join("nested/a.cif"), "").unwrap();
        fs::write(temp.path().join("nested/ignore.txt"), "").unwrap();
        fs::write(temp.path().join(".hidden/hidden.pdb"), "").unwrap();

        let browser = FileBrowser::open_directory(temp.path()).unwrap();
        let labels: Vec<&str> = browser
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(labels, ["nested/a.cif", "z.PDB"]);
        assert!(browser.visible);
        assert!(browser.focused);
    }

    #[test]
    fn selection_clamps_at_both_ends() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("a.pdb"), "").unwrap();
        fs::write(temp.path().join("b.pdb"), "").unwrap();
        let mut browser = FileBrowser::open_directory(temp.path()).unwrap();

        browser.move_selection(-1);
        assert_eq!(browser.selected, 0);
        browser.move_selection(10);
        assert_eq!(browser.selected, 1);
        browser.select_first();
        assert_eq!(browser.selected, 0);
        browser.select_last();
        assert_eq!(browser.selected, 1);
    }

    #[test]
    fn empty_directory_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let error = FileBrowser::open_directory(temp.path()).unwrap_err();
        assert!(error.to_string().contains("no PDB"));
    }

    #[test]
    fn panel_width_preserves_a_minimum_viewport() {
        assert_eq!(panel_width(100, true), PANEL_WIDTH);
        assert_eq!(panel_width(40, true), 20);
        assert_eq!(panel_width(15, true), 0);
        assert_eq!(panel_width(100, false), 0);
    }
}
