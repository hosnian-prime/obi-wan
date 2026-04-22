use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::List;
use ratatui::Frame;

use crate::widgets::editor::EditorWidget;

#[derive(Debug)]
struct TreeEntry {
    name: String,
    path: PathBuf,
    depth: usize,
    is_dir: bool,
    expanded: bool,
}

/// Patterns to always ignore in the file tree.
const HARDCODED_IGNORE: &[&str] = &["target", "node_modules", ".git", ".obi"];

pub struct FileTreeWidget {
    entries: Vec<TreeEntry>,
    selected: usize,
    gitignore_patterns: Vec<String>,
    root: PathBuf,
}

impl FileTreeWidget {
    pub fn new(root: &Path) -> Self {
        let gitignore_patterns = Self::load_gitignore(root);
        let mut entries = Vec::new();
        Self::scan_dir(root, &mut entries, 0, &gitignore_patterns);
        Self {
            entries,
            selected: 0,
            gitignore_patterns,
            root: root.to_path_buf(),
        }
    }

    fn load_gitignore(root: &Path) -> Vec<String> {
        let gitignore_path = root.join(".gitignore");
        match std::fs::read_to_string(gitignore_path) {
            Ok(content) => content
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.trim_end_matches('/').to_string())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn should_ignore(name: &str, gitignore: &[String]) -> bool {
        if name.starts_with('.') {
            return true;
        }
        if HARDCODED_IGNORE.contains(&name) {
            return true;
        }
        for pattern in gitignore {
            // Simple glob: exact match or wildcard prefix
            if pattern == name {
                return true;
            }
            if let Some(suffix) = pattern.strip_prefix('*') {
                if name.ends_with(suffix) {
                    return true;
                }
            }
        }
        false
    }

    fn scan_dir(dir: &Path, entries: &mut Vec<TreeEntry>, depth: usize, gitignore: &[String]) {
        let mut children: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    !Self::should_ignore(&name, gitignore)
                })
                .collect(),
            Err(_) => return,
        };

        // Directories first, then alphabetical
        children.sort_by(|a, b| {
            let a_dir = a.path().is_dir();
            let b_dir = b.path().is_dir();
            b_dir
                .cmp(&a_dir)
                .then_with(|| a.file_name().cmp(&b.file_name()))
        });

        for child in children {
            let path = child.path();
            let is_dir = path.is_dir();
            entries.push(TreeEntry {
                name: child.file_name().to_string_lossy().to_string(),
                path,
                depth,
                is_dir,
                expanded: false,
            });
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, editor: &mut EditorWidget) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let count = self.visible_count();
                if count > 0 && self.selected + 1 < count {
                    self.selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Right => {
                self.toggle_or_open(editor);
            }
            KeyCode::Left => {
                // Collapse current directory or go to parent
                let visible: Vec<_> = self.visible_indices();
                if let Some(&idx) = visible.get(self.selected) {
                    if self.entries[idx].is_dir && self.entries[idx].expanded {
                        self.entries[idx].expanded = false;
                        self.remove_children(idx);
                    }
                }
            }
            _ => {}
        }
    }

    /// Scroll selection up by one.
    pub fn scroll_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Scroll selection down by one.
    pub fn scroll_down(&mut self) {
        let count = self.visible_count();
        if count > 0 && self.selected + 1 < count {
            self.selected += 1;
        }
    }

    fn visible_count(&self) -> usize {
        self.visible_indices().len()
    }

    /// Returns flat indices into self.entries for all currently visible entries.
    fn visible_indices(&self) -> Vec<usize> {
        let mut result = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            let entry = &self.entries[i];
            if entry.depth == 0 {
                result.push(i);
                if entry.is_dir && !entry.expanded {
                    // Skip all children
                    i = self.skip_children(i);
                    continue;
                }
            } else {
                // Entry is visible if we can see it (it was added by expansion)
                // Since we insert/remove children dynamically, all entries present
                // with depth > 0 are visible IF their parent chain is expanded.
                result.push(i);
                if entry.is_dir && !entry.expanded {
                    i = self.skip_children(i);
                    continue;
                }
            }
            i += 1;
        }
        result
    }

    fn skip_children(&self, parent_idx: usize) -> usize {
        let parent_depth = self.entries[parent_idx].depth;
        let mut i = parent_idx + 1;
        while i < self.entries.len() && self.entries[i].depth > parent_depth {
            i += 1;
        }
        i
    }

    fn toggle_or_open(&mut self, editor: &mut EditorWidget) {
        let visible = self.visible_indices();
        if let Some(&entry_idx) = visible.get(self.selected) {
            if self.entries[entry_idx].is_dir {
                let was_expanded = self.entries[entry_idx].expanded;
                self.entries[entry_idx].expanded = !was_expanded;

                if !was_expanded {
                    let depth = self.entries[entry_idx].depth + 1;
                    let dir_path = self.entries[entry_idx].path.clone();
                    self.remove_children(entry_idx);
                    let mut children = Vec::new();
                    Self::scan_dir(&dir_path, &mut children, depth, &self.gitignore_patterns);
                    for (i, child) in children.into_iter().enumerate() {
                        self.entries.insert(entry_idx + 1 + i, child);
                    }
                } else {
                    self.remove_children(entry_idx);
                }
            } else {
                let path = self.entries[entry_idx].path.clone();
                editor.open_file(&path);
            }
        }
    }

    fn remove_children(&mut self, parent_idx: usize) {
        let parent_depth = self.entries[parent_idx].depth;
        let mut remove_count = 0;
        for entry in self.entries.iter().skip(parent_idx + 1) {
            if entry.depth > parent_depth {
                remove_count += 1;
            } else {
                break;
            }
        }
        if remove_count > 0 {
            self.entries
                .drain((parent_idx + 1)..(parent_idx + 1 + remove_count));
        }
    }

    /// Handle a mouse click at the given row offset within the widget area.
    /// Returns true if a file was opened (so the caller can switch focus to editor).
    pub fn handle_click(&mut self, row: u16, editor: &mut EditorWidget) -> bool {
        let visible = self.visible_indices();
        let clicked = row as usize;
        if clicked < visible.len() {
            self.selected = clicked;
            // Open file or toggle directory
            if let Some(&entry_idx) = visible.get(self.selected) {
                let is_file = !self.entries[entry_idx].is_dir;
                self.toggle_or_open(editor);
                return is_file;
            }
        }
        false
    }

    /// Returns all file paths for command palette fuzzy search.
    pub fn all_file_paths(&self) -> Vec<PathBuf> {
        self.collect_files_recursive(&self.root)
    }

    fn collect_files_recursive(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let rd = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return files,
        };
        for entry in rd.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if Self::should_ignore(&name, &self.gitignore_patterns) {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                files.extend(self.collect_files_recursive(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let visible = self.visible_indices();
        let items: Vec<Line> = visible
            .iter()
            .enumerate()
            .map(|(vis_idx, &entry_idx)| {
                let entry = &self.entries[entry_idx];
                let indent = "  ".repeat(entry.depth);
                let icon = if entry.is_dir {
                    if entry.expanded {
                        "v "
                    } else {
                        "> "
                    }
                } else {
                    "  "
                };
                let style = if vis_idx == self.selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if entry.is_dir {
                    Style::default().fg(Color::Blue)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(
                    format!("{indent}{icon}{}", entry.name),
                    style,
                ))
            })
            .collect();

        let list = List::new(items);
        frame.render_widget(list, area);
    }
}
