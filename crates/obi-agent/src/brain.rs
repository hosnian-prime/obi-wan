use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::OnceCell;

use obi_core::config::{global_obi_dir, BrainConfig, ObiConfig};
use obi_core::graph::KnowledgeGraph;
use obi_core::node::NodeId;
use obi_indexer::store::VectorStore;

/// A single brain instance (project or global).
pub struct BrainInstance {
    pub graph: KnowledgeGraph,
    pub db_path: PathBuf,
    pub obi_dir: PathBuf,
}

/// Result from dual-brain vector search, with source tracking.
#[derive(Debug, Clone)]
pub struct DualSearchResult {
    pub id: String,
    pub content: String,
    /// Score already boosted (project results × project_boost).
    pub score: f32,
    /// Whether this came from the project brain.
    pub is_project: bool,
}

/// DualBrain — manages project (.obi/) and global (~/.obi/) brains.
///
/// At query time:
/// 1. Search both vector stores
/// 2. Boost project results by project_boost multiplier
/// 3. Merge and deduplicate by node_id (project takes priority)
/// 4. Provide merged graph for BFS expansion
pub struct DualBrain {
    project: Option<BrainInstance>,
    global: Option<BrainInstance>,
    config: BrainConfig,
    /// Merged graph (project + global) for context expansion.
    merged_graph: KnowledgeGraph,
    /// Cached VectorStore for project brain (opened lazily on first use).
    project_store: OnceCell<Option<VectorStore>>,
    /// Cached VectorStore for global brain (opened lazily on first use).
    global_store: OnceCell<Option<VectorStore>>,
}

impl DualBrain {
    /// Load both brains based on config. Non-fatal: if a brain doesn't exist, it's skipped.
    pub fn load(project_root: &Path, config: &ObiConfig) -> Self {
        let brain_config = &config.brain;
        let mut merged_graph = KnowledgeGraph::new();

        // Load project brain
        let project = if brain_config.project_enabled {
            let obi_dir = project_root.join(".obi");
            let graph_path = obi_dir.join("graph.bin");
            let db_path = obi_dir.join("db");

            if graph_path.exists() && db_path.exists() {
                match KnowledgeGraph::load_from_disk(&graph_path) {
                    Ok(graph) => {
                        merged_graph.merge(&graph);
                        Some(BrainInstance {
                            graph,
                            db_path,
                            obi_dir,
                        })
                    }
                    Err(_) => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        // Load global brain
        let global = if brain_config.global_enabled {
            let obi_dir = global_obi_dir();
            let graph_path = obi_dir.join("graph.bin");
            let db_path = obi_dir.join("db");

            if graph_path.exists() && db_path.exists() {
                match KnowledgeGraph::load_from_disk(&graph_path) {
                    Ok(graph) => {
                        merged_graph.merge(&graph);
                        Some(BrainInstance {
                            graph,
                            db_path,
                            obi_dir,
                        })
                    }
                    Err(_) => None,
                }
            } else {
                None
            }
        } else {
            None
        };

        Self {
            project,
            global,
            config: brain_config.clone(),
            merged_graph,
            project_store: OnceCell::new(),
            global_store: OnceCell::new(),
        }
    }

    /// Get the merged knowledge graph (for context BFS expansion).
    pub fn merged_graph(&self) -> &KnowledgeGraph {
        &self.merged_graph
    }

    /// Get the merged graph as an Arc (for agent sharing).
    pub fn merged_graph_arc(&self) -> Arc<KnowledgeGraph> {
        Arc::new(self.merged_graph.clone())
    }

    /// Whether any brain is available.
    pub fn has_any_brain(&self) -> bool {
        self.project.is_some() || self.global.is_some()
    }

    /// Get or lazily open the cached project VectorStore.
    async fn project_store(&self, embedding_dims: usize) -> Option<&VectorStore> {
        let store = self
            .project_store
            .get_or_init(|| async {
                if let Some(ref project) = self.project {
                    if let Ok(store) = VectorStore::open(&project.db_path, embedding_dims).await {
                        if store.ensure_table().await.is_ok() {
                            return Some(store);
                        }
                    }
                }
                None
            })
            .await;
        store.as_ref()
    }

    /// Get or lazily open the cached global VectorStore.
    async fn global_store(&self, embedding_dims: usize) -> Option<&VectorStore> {
        let store = self
            .global_store
            .get_or_init(|| async {
                if let Some(ref global) = self.global {
                    if let Ok(store) = VectorStore::open(&global.db_path, embedding_dims).await {
                        if store.ensure_table().await.is_ok() {
                            return Some(store);
                        }
                    }
                }
                None
            })
            .await;
        store.as_ref()
    }

    /// Search both brains, apply project boost, merge and deduplicate.
    ///
    /// Returns combined results sorted by score descending, deduplicated by node_id
    /// (project brain takes priority on conflicts).
    pub async fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        embedding_dims: usize,
    ) -> Result<Vec<DualSearchResult>> {
        let mut all_results: HashMap<String, DualSearchResult> = HashMap::new();

        // Search both brains in parallel
        let project_fut = async {
            if let Some(store) = self.project_store(embedding_dims).await {
                store.search(query_embedding, top_k).await.ok()
            } else {
                None
            }
        };

        let global_fut = async {
            if let Some(store) = self.global_store(embedding_dims).await {
                store.search(query_embedding, top_k).await.ok()
            } else {
                None
            }
        };

        let (project_results, global_results) = tokio::join!(project_fut, global_fut);

        // Apply project boost
        if let Some(results) = project_results {
            for sr in results {
                let boosted_score = sr.score * self.config.project_boost as f32;
                all_results.insert(
                    sr.id.clone(),
                    DualSearchResult {
                        id: sr.id,
                        content: sr.content,
                        score: boosted_score,
                        is_project: true,
                    },
                );
            }
        }

        // Global results (no boost, project takes priority)
        if let Some(results) = global_results {
            for sr in results {
                all_results.entry(sr.id.clone()).or_insert(DualSearchResult {
                    id: sr.id,
                    content: sr.content,
                    score: sr.score,
                    is_project: false,
                });
            }
        }

        let mut results: Vec<DualSearchResult> = all_results.into_values().collect();
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);

        Ok(results)
    }

    /// Batch get content for multiple nodes. Single query per brain instead of N individual lookups.
    pub async fn get_contents_batch(
        &self,
        ids: &[NodeId],
        embedding_dims: usize,
    ) -> Result<HashMap<NodeId, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mut results = HashMap::new();

        // Batch query project brain
        if let Some(store) = self.project_store(embedding_dims).await {
            if let Ok(batch) = store.get_contents_batch(ids).await {
                results.extend(batch);
            }
        }

        // Batch query global brain for any remaining IDs
        let missing: Vec<NodeId> = ids
            .iter()
            .filter(|id| !results.contains_key(id))
            .copied()
            .collect();

        if !missing.is_empty() {
            if let Some(store) = self.global_store(embedding_dims).await {
                if let Ok(batch) = store.get_contents_batch(&missing).await {
                    results.extend(batch);
                }
            }
        }

        Ok(results)
    }

    /// Get content for a single node (uses cached stores).
    pub async fn get_content(&self, id: &NodeId, embedding_dims: usize) -> Result<Option<String>> {
        // Try project brain first
        if let Some(store) = self.project_store(embedding_dims).await {
            if let Ok(Some(content)) = store.get_content(id).await {
                return Ok(Some(content));
            }
        }

        // Fall back to global brain
        if let Some(store) = self.global_store(embedding_dims).await {
            if let Ok(Some(content)) = store.get_content(id).await {
                return Ok(Some(content));
            }
        }

        Ok(None)
    }

    /// Get stats for both brains.
    pub fn stats(&self) -> BrainStats {
        let project_stats = self.project.as_ref().map(|b| SingleBrainStats {
            node_count: b.graph.node_count(),
            edge_count: b.graph.edge_count(),
            obi_dir: b.obi_dir.clone(),
        });

        let global_stats = self.global.as_ref().map(|b| SingleBrainStats {
            node_count: b.graph.node_count(),
            edge_count: b.graph.edge_count(),
            obi_dir: b.obi_dir.clone(),
        });

        BrainStats {
            project: project_stats,
            global: global_stats,
            merged_node_count: self.merged_graph.node_count(),
            merged_edge_count: self.merged_graph.edge_count(),
        }
    }
}

#[derive(Debug)]
pub struct SingleBrainStats {
    pub node_count: usize,
    pub edge_count: usize,
    pub obi_dir: PathBuf,
}

#[derive(Debug)]
pub struct BrainStats {
    pub project: Option<SingleBrainStats>,
    pub global: Option<SingleBrainStats>,
    pub merged_node_count: usize,
    pub merged_edge_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use obi_core::config::ObiConfig;

    #[test]
    fn test_dual_brain_load_nonexistent() {
        let config = ObiConfig::default();
        let brain = DualBrain::load(Path::new("/nonexistent/path"), &config);
        assert!(!brain.has_any_brain());
        assert_eq!(brain.merged_graph().node_count(), 0);
    }

    #[test]
    fn test_dual_brain_disabled() {
        let mut config = ObiConfig::default();
        config.brain.project_enabled = false;
        config.brain.global_enabled = false;
        let brain = DualBrain::load(Path::new("/tmp"), &config);
        assert!(!brain.has_any_brain());
    }

    #[test]
    fn test_brain_stats_empty() {
        let config = ObiConfig::default();
        let brain = DualBrain::load(Path::new("/nonexistent"), &config);
        let stats = brain.stats();
        assert!(stats.project.is_none());
        assert!(stats.global.is_none());
        assert_eq!(stats.merged_node_count, 0);
    }
}
