mod app;
mod event;
mod layout;
mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use obi_llm::provider::EmbeddingProvider;
use tokio::sync::mpsc;

use app::App;

#[derive(Parser)]
#[command(name = "obi", about = "AI-Native TUI IDE with knowledge graph")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Full re-index of the current project
    Index,
    /// Show index stats (node count, edge count, files indexed)
    Status,
    /// Show dual-brain status (project + global brain stats)
    Brain,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Index) => cmd_index().await,
        Some(Command::Status) => cmd_status().await,
        Some(Command::Brain) => cmd_brain_status().await,
        None => run_tui().await,
    }
}

/// Launch the TUI editor (default when no subcommand is given).
async fn run_tui() -> Result<()> {
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Install panic hook that restores terminal BEFORE printing panic info.
    let original_hook = std::panic::take_hook();
    let pr = project_root.clone();
    let pr2 = project_root.clone();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort restore — ignore errors
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::cursor::Show
        );
        // Write crash log to .obi/crash.log for debugging
        let crash_msg = format!("{info}");
        let crash_path = pr.join(".obi").join("crash.log");
        let _ = std::fs::write(&crash_path, &crash_msg);
        eprintln!("\n[obi crash log written to {}]", crash_path.display());
        original_hook(info);
    }));

    // Enable mouse capture and bracketed paste alongside raw mode
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableMouseCapture,
        crossterm::event::EnableBracketedPaste
    )?;
    let terminal = ratatui::init();
    let mut app = App::new(pr2.clone());
    let result = app.run(terminal).await;
    ratatui::restore();
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste
    )?;

    if let Err(ref e) = result {
        let crash_path = pr2.join(".obi").join("crash.log");
        let msg = format!("App error: {e:#}");
        let _ = std::fs::write(&crash_path, &msg);
        eprintln!("[obi error log written to {}]", crash_path.display());
    }

    result
}

/// Progress reporting trait — abstracts CLI println vs TUI event sending.
trait IndexProgress: Send + Sync {
    fn status(&self, msg: &str);
    fn progress(&self, done: usize, total: usize);
    fn warn(&self, msg: &str);
    fn is_cancelled(&self) -> bool;
}

/// CLI progress reporter — prints to stdout/stderr.
struct CliProgress;

impl IndexProgress for CliProgress {
    fn status(&self, msg: &str) { println!("  {msg}"); }
    fn progress(&self, _done: usize, _total: usize) {}
    fn warn(&self, msg: &str) { eprintln!("  Warning: {msg}"); }
    fn is_cancelled(&self) -> bool { false }
}

/// TUI progress reporter — sends events to the app.
struct TuiProgress<'a> {
    tx: &'a mpsc::UnboundedSender<event::AppEvent>,
    cancelled: &'a AtomicBool,
}

impl IndexProgress for TuiProgress<'_> {
    fn status(&self, msg: &str) {
        let _ = self.tx.send(event::AppEvent::IndexingStatus(msg.to_string()));
    }
    fn progress(&self, done: usize, total: usize) {
        let _ = self.tx.send(event::AppEvent::IndexingProgress { done, total });
    }
    fn warn(&self, msg: &str) {
        let _ = self.tx.send(event::AppEvent::IndexingStatus(format!("Warning: {msg}")));
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// Shared indexing pipeline used by both CLI (`obi index`) and TUI (startup dialog).
async fn index_core(
    project_root: &Path,
    progress: &dyn IndexProgress,
) -> Result<()> {
    let obi_dir = project_root.join(".obi");
    std::fs::create_dir_all(&obi_dir)?;

    let graph_path = obi_dir.join("graph.bin");
    let db_path = obi_dir.join("db");

    let obi_config = obi_core::config::ObiConfig::load(project_root);
    let embedding_enabled = obi_config.brain.embedding_enabled;

    progress.status("Parsing files...");

    // Step 1: Parse
    let config = obi_indexer::IndexConfig::default();
    let mut result = obi_indexer::index_directory(project_root, &config)?;

    progress.status(&format!(
        "Parsed {} files → {} nodes, {} edges",
        result.files_indexed, result.graph.node_count(), result.graph.edge_count(),
    ));

    if progress.is_cancelled() { anyhow::bail!("Cancelled"); }

    // Step 2: Diff
    let old_graph = if graph_path.exists() {
        obi_core::graph::KnowledgeGraph::load_from_disk(&graph_path).ok()
    } else {
        None
    };

    let nodes_to_embed: HashMap<obi_core::node::NodeId, String>;
    let mut removed_ids: Vec<obi_core::node::NodeId> = Vec::new();

    if let Some(ref old) = old_graph {
        let old_hashes: HashMap<String, [u8; 32]> = old
            .nodes()
            .map(|n| (format!("{}:{}", n.file_path.display(), n.name), n.content_hash))
            .collect();

        nodes_to_embed = result.node_contents.iter()
            .filter(|(id, _)| {
                let node = result.graph.get_node(id).unwrap();
                let key = format!("{}:{}", node.file_path.display(), node.name);
                match old_hashes.get(&key) {
                    Some(old_hash) => *old_hash != node.content_hash,
                    None => true,
                }
            })
            .map(|(id, content)| (*id, content.clone()))
            .collect();

        let new_keys: HashSet<String> = result.graph.nodes()
            .map(|n| format!("{}:{}", n.file_path.display(), n.name))
            .collect();

        removed_ids = old.nodes()
            .filter(|n| !new_keys.contains(&format!("{}:{}", n.file_path.display(), n.name)))
            .map(|n| n.id)
            .collect();

        progress.status(&format!(
            "Incremental: {} new/modified, {} unchanged, {} removed",
            nodes_to_embed.len(),
            result.node_contents.len() - nodes_to_embed.len(),
            removed_ids.len(),
        ));
    } else {
        nodes_to_embed = result.node_contents.clone();
        progress.status(&format!("First index: embedding all {} nodes", nodes_to_embed.len()));
    }

    // Steps 3-5: Embedding pipeline (skipped when disabled)
    if embedding_enabled {
        let provider = obi_llm::ollama::OllamaEmbedding::default_local();
        let total = nodes_to_embed.len();
        progress.progress(0, total);
        progress.status("Embedding nodes...");

        if progress.is_cancelled() { anyhow::bail!("Cancelled"); }

        let mut embeddings = HashMap::new();
        if !nodes_to_embed.is_empty() {
            let truncated: Vec<(obi_core::node::NodeId, String)> = nodes_to_embed.iter()
                .map(|(id, content)| {
                    let text = if content.len() > 30_000 { content[..30_000].to_string() } else { content.clone() };
                    (*id, text)
                })
                .collect();

            let batch_size = provider.max_batch_size();
            let mut done = 0_usize;

            for chunk in truncated.chunks(batch_size) {
                if progress.is_cancelled() { anyhow::bail!("Cancelled"); }

                let texts: Vec<&str> = chunk.iter().map(|(_, c)| c.as_str()).collect();
                let ids: Vec<obi_core::node::NodeId> = chunk.iter().map(|(id, _)| *id).collect();

                match provider.embed(&texts).await {
                    Ok(vecs) => {
                        for (id, vec) in ids.into_iter().zip(vecs) {
                            embeddings.insert(id, vec);
                        }
                    }
                    Err(_) => {
                        for (id, content) in chunk {
                            if let Ok(vecs) = provider.embed(&[content.as_str()]).await {
                                if let Some(vec) = vecs.into_iter().next() {
                                    embeddings.insert(*id, vec);
                                }
                            }
                        }
                    }
                }

                done += chunk.len();
                progress.progress(done, total);
            }
        }

        progress.status("Storing in database...");

        if !embeddings.is_empty() || !removed_ids.is_empty() {
            match obi_indexer::store::VectorStore::open(&db_path, provider.dimensions()).await {
                Ok(store) => {
                    if !removed_ids.is_empty() {
                        if let Err(e) = store.delete(&removed_ids).await {
                            progress.warn(&format!("failed to delete removed nodes: {e}"));
                        }
                    }
                    if !embeddings.is_empty() {
                        let records: Vec<obi_indexer::store::NodeRecord> = embeddings.iter()
                            .filter_map(|(id, embedding)| {
                                let node = result.graph.get_node(id)?;
                                let content = result.node_contents.get(id)?;
                                Some(obi_indexer::store::NodeRecord {
                                    id: id.to_string(),
                                    name: node.name.clone(),
                                    content: content.clone(),
                                    file_path: node.file_path.to_string_lossy().to_string(),
                                    node_type: format!("{:?}", node.node_type),
                                    embedding: embedding.clone(),
                                })
                            })
                            .collect();
                        if let Err(e) = store.upsert(&records).await {
                            progress.warn(&format!("LanceDB upsert failed: {e}"));
                        } else {
                            progress.status(&format!("Stored {} records in LanceDB", records.len()));
                        }
                    }
                }
                Err(e) => progress.warn(&format!("failed to open LanceDB: {e}")),
            }
        }

        progress.status("Resolving semantic edges...");
        if !embeddings.is_empty() {
            let node_ids: Vec<obi_core::node::NodeId> = result.graph.all_nodes().keys().copied().collect();
            let semantic_edges = obi_indexer::edges::resolve_semantic_edges(&node_ids, &embeddings, 0.85);
            for edge in semantic_edges {
                result.graph.add_edge(edge);
            }
        }
    } else {
        progress.status("Graph-only mode (embedding disabled)");
        progress.progress(result.graph.node_count(), result.graph.node_count());
    }

    // Step 6: Save graph
    progress.status("Saving graph...");
    result.graph.save_to_disk(&graph_path)?;

    // Step 7: Timestamp
    let timestamp = chrono::Utc::now().to_rfc3339();
    let _ = std::fs::write(obi_dir.join("last_indexed"), &timestamp);

    progress.status(&format!(
        "Done. {} nodes, {} edges",
        result.graph.node_count(), result.graph.edge_count(),
    ));

    Ok(())
}

/// `obi index` — CLI wrapper around shared indexing pipeline.
async fn cmd_index() -> Result<()> {
    let project_root = std::env::current_dir()?;
    println!("Indexing {}...", project_root.display());
    index_core(&project_root, &CliProgress).await?;
    Ok(())
}

/// `obi status` — show index stats.
async fn cmd_status() -> Result<()> {
    let project_root = std::env::current_dir()?;
    let obi_dir = project_root.join(".obi");
    let graph_path = obi_dir.join("graph.bin");

    if !graph_path.exists() {
        println!("No index found. Run `obi index` first.");
        return Ok(());
    }

    let graph = obi_core::graph::KnowledgeGraph::load_from_disk(&graph_path)?;

    println!("Project: {}", project_root.display());
    println!("Nodes:   {}", graph.node_count());
    println!("Edges:   {}", graph.edge_count());

    // Count by node type
    let mut type_counts = std::collections::HashMap::new();
    for node in graph.nodes() {
        *type_counts
            .entry(format!("{:?}", node.node_type))
            .or_insert(0usize) += 1;
    }
    let mut sorted: Vec<_> = type_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    println!("\nBy type:");
    for (ntype, count) in &sorted {
        println!("  {:<12} {}", ntype, count);
    }

    // Count by language
    let mut lang_counts = std::collections::HashMap::new();
    for node in graph.nodes() {
        *lang_counts
            .entry(format!("{:?}", node.language))
            .or_insert(0usize) += 1;
    }
    let mut sorted: Vec<_> = lang_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    println!("\nBy language:");
    for (lang, count) in &sorted {
        println!("  {:<12} {}", lang, count);
    }

    // LanceDB stats
    let db_path = obi_dir.join("db");
    if db_path.exists() {
        match obi_indexer::store::VectorStore::open(&db_path, 768).await {
            Ok(store) => match store.count().await {
                Ok(count) => println!("\nLanceDB: {} embedded records", count),
                Err(_) => println!("\nLanceDB: present but could not read count"),
            },
            Err(_) => {}
        }
    }

    Ok(())
}

/// `obi brain` — show dual-brain status (project + global).
async fn cmd_brain_status() -> Result<()> {
    let project_root = std::env::current_dir()?;
    let config = obi_core::config::ObiConfig::load(&project_root);
    let brain = obi_agent::brain::DualBrain::load(&project_root, &config);
    let stats = brain.stats();

    println!("=== Dual Brain Status ===\n");

    // Project brain
    match &stats.project {
        Some(ps) => {
            println!("Project brain: {}", ps.obi_dir.display());
            println!("  Nodes: {}", ps.node_count);
            println!("  Edges: {}", ps.edge_count);

            // Count by node type
            let graph_path = ps.obi_dir.join("graph.bin");
            if let Ok(graph) = obi_core::graph::KnowledgeGraph::load_from_disk(&graph_path) {
                let mut type_counts = HashMap::new();
                for node in graph.nodes() {
                    *type_counts.entry(format!("{:?}", node.node_type)).or_insert(0usize) += 1;
                }
                let mut sorted: Vec<_> = type_counts.into_iter().collect();
                sorted.sort_by(|a, b| b.1.cmp(&a.1));
                for (ntype, count) in &sorted {
                    println!("    {:<12} {}", ntype, count);
                }
            }

            // LanceDB count
            let db_path = ps.obi_dir.join("db");
            if db_path.exists() {
                if let Ok(store) = obi_indexer::store::VectorStore::open(&db_path, 768).await {
                    if let Ok(count) = store.count().await {
                        println!("  LanceDB: {} embedded records", count);
                    }
                }
            }

            // Last indexed timestamp
            let ts_path = ps.obi_dir.join("last_indexed");
            if let Ok(ts) = std::fs::read_to_string(&ts_path) {
                println!("  Last indexed: {}", format_timestamp(&ts));
            }
        }
        None => {
            if config.brain.project_enabled {
                println!("Project brain: not indexed (run `obi index`)");
            } else {
                println!("Project brain: disabled");
            }
        }
    }

    println!();

    // Global brain
    match &stats.global {
        Some(gs) => {
            println!("Global brain: {}", gs.obi_dir.display());
            println!("  Nodes: {}", gs.node_count);
            println!("  Edges: {}", gs.edge_count);

            let db_path = gs.obi_dir.join("db");
            if db_path.exists() {
                if let Ok(store) = obi_indexer::store::VectorStore::open(&db_path, 768).await {
                    if let Ok(count) = store.count().await {
                        println!("  LanceDB: {} embedded records", count);
                    }
                }
            }

            // Last indexed timestamp
            let ts_path = gs.obi_dir.join("last_indexed");
            if let Ok(ts) = std::fs::read_to_string(&ts_path) {
                println!("  Last indexed: {}", format_timestamp(&ts));
            }
        }
        None => {
            if config.brain.global_enabled {
                println!("Global brain: not found (~/.obi/)");
            } else {
                println!("Global brain: disabled");
            }
        }
    }

    println!();
    println!("Merged: {} nodes, {} edges", stats.merged_node_count, stats.merged_edge_count);
    println!("Project boost: {}x", config.brain.project_boost);

    Ok(())
}

/// TUI wrapper — runs shared indexing pipeline with event-based progress.
pub(crate) async fn run_index_with_progress(
    project_root: PathBuf,
    tx: mpsc::UnboundedSender<event::AppEvent>,
    cancelled: Arc<AtomicBool>,
) {
    let progress = TuiProgress { tx: &tx, cancelled: &cancelled };
    let result = index_core(&project_root, &progress).await;
    match result {
        Ok(()) => { let _ = tx.send(event::AppEvent::IndexingComplete); }
        Err(e) => {
            if !cancelled.load(Ordering::Relaxed) {
                let _ = tx.send(event::AppEvent::IndexingError(e.to_string()));
            }
        }
    }
}

/// Format a stored RFC3339 timestamp into a human-readable relative time.
fn format_timestamp(ts: &str) -> String {
    use chrono::{DateTime, Utc};
    match ts.trim().parse::<DateTime<Utc>>() {
        Ok(dt) => {
            let now = Utc::now();
            let diff = now.signed_duration_since(dt);
            let secs = diff.num_seconds();
            if secs < 60 {
                "just now".to_string()
            } else if secs < 3600 {
                format!("{} minutes ago", secs / 60)
            } else if secs < 86400 {
                format!("{} hours ago", secs / 3600)
            } else {
                format!("{} days ago", secs / 86400)
            }
        }
        Err(_) => ts.trim().to_string(), // fallback: show raw string
    }
}
