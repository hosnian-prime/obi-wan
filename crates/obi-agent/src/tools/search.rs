use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolPermission, ToolResult};

/// Full-text search across project code AND notes (both brains).
/// Searches project root + global brain notes (~/.obi/notes/).
pub struct SearchTool {
    project_root: PathBuf,
    /// Additional directories to search (e.g., global brain notes).
    extra_search_dirs: Vec<PathBuf>,
}

impl SearchTool {
    pub fn new(project_root: PathBuf) -> Self {
        // Add global brain notes directory to search scope
        let global_notes = obi_core::config::global_obi_dir().join("notes");
        let mut extra_search_dirs = Vec::new();
        if global_notes.exists() {
            extra_search_dirs.push(global_notes);
        }

        Self {
            project_root,
            extra_search_dirs,
        }
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search for a pattern in files within the project and global brain notes. \
         Uses regular expression matching. \
         Returns matching lines with file paths and line numbers. \
         Optionally filter by file extension."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Search pattern (regular expression)"
                },
                "file_extension": {
                    "type": "string",
                    "description": "Optional file extension filter (e.g. 'rs', 'py', 'ts', 'md')"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matching lines to return (default: 30)"
                }
            }
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let pattern = args
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'pattern' argument"))?;

        let file_ext = args.get("file_extension").and_then(|v| v.as_str());
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(30) as usize;

        let project_root = self.project_root.clone();
        let extra_dirs = self.extra_search_dirs.clone();
        let pattern = pattern.to_string();
        let file_ext = file_ext.map(|s| s.to_string());

        // Run grep in a blocking task to avoid blocking the async runtime
        let result = tokio::task::spawn_blocking(move || {
            search_files(&project_root, &extra_dirs, &pattern, file_ext.as_deref(), max_results)
        })
        .await??;

        Ok(result)
    }
}

fn search_files(
    root: &PathBuf,
    extra_dirs: &[PathBuf],
    pattern: &str,
    file_ext: Option<&str>,
    max_results: usize,
) -> Result<ToolResult> {
    let regex = match regex::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            return Ok(ToolResult {
                output: format!("Invalid regex pattern: {}", e),
                success: false,
            });
        }
    };

    let mut results = Vec::new();

    // Search project root
    search_directory(root, root, &regex, file_ext, max_results, &mut results);

    // Search extra directories (global brain notes, etc.)
    for extra_dir in extra_dirs {
        if results.len() >= max_results {
            break;
        }
        search_directory(extra_dir, extra_dir, &regex, file_ext, max_results, &mut results);
    }

    if results.is_empty() {
        Ok(ToolResult {
            output: format!("No matches found for pattern '{}'", pattern),
            success: true,
        })
    } else {
        let total = results.len();
        Ok(ToolResult {
            output: format!("{} match(es):\n{}", total, results.join("\n")),
            success: true,
        })
    }
}

fn search_directory(
    dir: &PathBuf,
    display_root: &PathBuf,
    regex: &regex::Regex,
    file_ext: Option<&str>,
    max_results: usize,
    results: &mut Vec<String>,
) {
    use std::io::BufRead;

    // Skip directories that are commonly ignored
    let skip_dirs = [".git", ".obi", "target", "node_modules", ".venv", "__pycache__"];

    let mut dirs = vec![dir.clone()];

    while let Some(dir) = dirs.pop() {
        if results.len() >= max_results {
            break;
        }

        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = path.file_name().unwrap_or_default().to_string_lossy();
                if !skip_dirs.iter().any(|&s| dir_name == s) {
                    dirs.push(path);
                }
            } else if path.is_file() {
                // Filter by extension if specified
                if let Some(ext) = file_ext {
                    match path.extension() {
                        Some(e) if e == ext => {}
                        _ => continue,
                    }
                }

                // Skip binary files
                if let Some(ext) = path.extension() {
                    let ext = ext.to_string_lossy();
                    if ["png", "jpg", "jpeg", "gif", "bin", "exe", "so", "dylib", "wasm"]
                        .contains(&ext.as_ref())
                    {
                        continue;
                    }
                }

                let file = match std::fs::File::open(&path) {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                // Show relative path from display_root, or prefix with [global] for extra dirs
                let display_path = if path.starts_with(display_root) {
                    let relative = path.strip_prefix(display_root).unwrap_or(&path);
                    if display_root != &dirs.first().cloned().unwrap_or_default() {
                        format!("[global] {}", relative.display())
                    } else {
                        relative.display().to_string()
                    }
                } else {
                    path.display().to_string()
                };

                let reader = std::io::BufReader::new(file);

                for (line_num, line) in reader.lines().enumerate() {
                    if results.len() >= max_results {
                        break;
                    }
                    if let Ok(line) = line {
                        if regex.is_match(&line) {
                            results.push(format!(
                                "{}:{}: {}",
                                display_path,
                                line_num + 1,
                                line.trim()
                            ));
                        }
                    }
                }

                if results.len() >= max_results {
                    break;
                }
            }
        }
    }
}
