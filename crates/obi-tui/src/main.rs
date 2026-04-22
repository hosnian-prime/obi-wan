mod app;
mod event;
mod layout;
mod widgets;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use obi_llm::provider::EmbeddingProvider;

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
            crossterm::cursor::Show
        );
        // Write crash log to .obi/crash.log for debugging
        let crash_msg = format!("{info}");
        let crash_path = pr.join(".obi").join("crash.log");
        let _ = std::fs::write(&crash_path, &crash_msg);
        eprintln!("\n[obi crash log written to {}]", crash_path.display());
        original_hook(info);
    }));

    // Enable mouse capture alongside raw mode
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableMouseCapture
    )?;
    let terminal = ratatui::init();
    let mut app = App::new(pr2.clone());
    let result = app.run(terminal).await;
    ratatui::restore();
    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture
    )?;

    if let Err(ref e) = result {
        let crash_path = pr2.join(".obi").join("crash.log");
        let msg = format!("App error: {e:#}");
        let _ = std::fs::write(&crash_path, &msg);
        eprintln!("[obi error log written to {}]", crash_path.display());
    }

    result
}

/// `obi index` — full re-index of the current project.
///
/// Pipeline (matching docs/02-architecture.md):
///   1. Parse all source files → KnowledgeGraph + node_contents
///   2. Diff against previous index (blake3 content hash) → find changed/new nodes
///   3. Embed only changed/new nodes via Ollama
///   4. Upsert into LanceDB
///   5. Resolve semantic edges (cosine similarity > 0.85)
///   6. Save graph to disk
async fn cmd_index() -> Result<()> {
    let project_root = std::env::current_dir()?;
    let obi_dir = project_root.join(".obi");
    std::fs::create_dir_all(&obi_dir)?;

    let graph_path = obi_dir.join("graph.bin");
    let db_path = obi_dir.join("db");

    println!("Indexing {}...", project_root.display());

    // --- Step 1: Parse all source files ---
    let config = obi_indexer::IndexConfig::default();
    let mut result = obi_indexer::index_directory(&project_root, &config)?;

    println!(
        "  Parsed {} files → {} nodes, {} edges",
        result.files_indexed,
        result.graph.node_count(),
        result.graph.edge_count(),
    );

    // --- Step 2: Diff against previous index ---
    let old_graph = if graph_path.exists() {
        obi_core::graph::KnowledgeGraph::load_from_disk(&graph_path).ok()
    } else {
        None
    };

    let nodes_to_embed: HashMap<obi_core::node::NodeId, String>;
    let mut removed_ids: Vec<obi_core::node::NodeId> = Vec::new();

    if let Some(ref old) = old_graph {
        // Build lookup: (file_path:name) → content_hash from old graph
        let old_hashes: HashMap<String, [u8; 32]> = old
            .nodes()
            .map(|n| {
                let key = format!("{}:{}", n.file_path.display(), n.name);
                (key, n.content_hash)
            })
            .collect();

        // Only embed nodes that are new or have changed content
        nodes_to_embed = result
            .node_contents
            .iter()
            .filter(|(id, _)| {
                let node = result.graph.get_node(id).unwrap();
                let key = format!("{}:{}", node.file_path.display(), node.name);
                match old_hashes.get(&key) {
                    Some(old_hash) => *old_hash != node.content_hash,
                    None => true, // new node
                }
            })
            .map(|(id, content)| (*id, content.clone()))
            .collect();

        // Find removed nodes (in old but not in new)
        let new_keys: HashSet<String> = result
            .graph
            .nodes()
            .map(|n| format!("{}:{}", n.file_path.display(), n.name))
            .collect();

        removed_ids = old
            .nodes()
            .filter(|n| !new_keys.contains(&format!("{}:{}", n.file_path.display(), n.name)))
            .map(|n| n.id)
            .collect();

        println!(
            "  Incremental: {} new/modified, {} unchanged, {} removed",
            nodes_to_embed.len(),
            result.node_contents.len() - nodes_to_embed.len(),
            removed_ids.len(),
        );
    } else {
        nodes_to_embed = result.node_contents.clone();
        println!("  First index: embedding all {} nodes", nodes_to_embed.len());
    }

    // --- Step 3: Embed via Ollama ---
    let provider = obi_llm::ollama::OllamaEmbedding::default_local();
    let embeddings = match obi_indexer::embedder::embed_nodes(&provider, &nodes_to_embed).await {
        Ok(emb) => {
            println!("  Embedded {} nodes via Ollama", emb.len());
            emb
        }
        Err(e) => {
            eprintln!(
                "  Warning: embedding failed ({}). Skipping LanceDB + semantic edges.",
                e
            );
            eprintln!("  Tip: make sure Ollama is running (`ollama serve`)");
            HashMap::new()
        }
    };

    // --- Step 4: Store in LanceDB ---
    if !embeddings.is_empty() || !removed_ids.is_empty() {
        match obi_indexer::store::VectorStore::open(&db_path, provider.dimensions()).await {
            Ok(store) => {
                // Delete removed nodes
                if !removed_ids.is_empty() {
                    if let Err(e) = store.delete(&removed_ids).await {
                        eprintln!("  Warning: failed to delete removed nodes from LanceDB: {}", e);
                    }
                }

                // Upsert new/modified nodes
                if !embeddings.is_empty() {
                    let records: Vec<obi_indexer::store::NodeRecord> = embeddings
                        .iter()
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

                    match store.upsert(&records).await {
                        Ok(()) => println!("  Stored {} records in LanceDB", records.len()),
                        Err(e) => eprintln!("  Warning: LanceDB upsert failed: {}", e),
                    }
                }
            }
            Err(e) => {
                eprintln!("  Warning: failed to open LanceDB: {}", e);
            }
        }
    }

    // --- Step 5: Resolve semantic edges ---
    if !embeddings.is_empty() {
        let node_ids: Vec<obi_core::node::NodeId> =
            result.graph.all_nodes().keys().copied().collect();
        let semantic_edges =
            obi_indexer::edges::resolve_semantic_edges(&node_ids, &embeddings, 0.85);
        let semantic_count = semantic_edges.len();
        for edge in semantic_edges {
            result.graph.add_edge(edge);
        }
        if semantic_count > 0 {
            println!("  Added {} semantic edges (cosine > 0.85)", semantic_count);
        }
    }

    // --- Step 6: Save graph ---
    result.graph.save_to_disk(&graph_path)?;

    // --- Step 7: Save last indexed timestamp ---
    let timestamp = chrono::Utc::now().to_rfc3339();
    let _ = std::fs::write(obi_dir.join("last_indexed"), &timestamp);

    println!(
        "\nDone. {} nodes, {} edges. Graph saved to {}",
        result.graph.node_count(),
        result.graph.edge_count(),
        graph_path.display(),
    );

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
