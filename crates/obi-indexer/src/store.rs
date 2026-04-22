use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};
use obi_core::node::NodeId;

/// LanceDB vector store for embeddings + content.
/// Stores the full content and embedding for each SemanticNode,
/// while the KnowledgeGraph holds topology only.
pub struct VectorStore {
    db: lancedb::Connection,
    dims: usize,
}

/// A record to upsert into the vector store.
pub struct NodeRecord {
    pub id: String,
    pub name: String,
    pub content: String,
    pub file_path: String,
    pub node_type: String,
    pub embedding: Vec<f32>,
}

/// A search result from vector similarity search.
#[derive(Debug)]
pub struct SearchResult {
    pub id: String,
    pub content: String,
    pub score: f32,
}

impl VectorStore {
    /// Open or create a LanceDB database at the given path.
    pub async fn open(db_path: &Path, dims: usize) -> Result<Self> {
        let db = lancedb::connect(db_path.to_str().unwrap_or(".obi/db"))
            .execute()
            .await
            .context("failed to open LanceDB")?;

        Ok(Self { db, dims })
    }

    fn schema(&self) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("file_path", DataType::Utf8, false),
            Field::new("node_type", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    self.dims as i32,
                ),
                false,
            ),
        ]))
    }

    /// Ensure the nodes table exists, creating it if needed.
    pub async fn ensure_table(&self) -> Result<lancedb::Table> {
        let table_names = self.db.table_names().execute().await?;

        if table_names.iter().any(|n| n == "nodes") {
            let table = self.db.open_table("nodes").execute().await?;
            Ok(table)
        } else {
            let schema = self.schema();
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
            let table = self
                .db
                .create_table("nodes", Box::new(batches))
                .execute()
                .await
                .context("failed to create nodes table")?;
            Ok(table)
        }
    }

    /// Build an arrow RecordBatch from node records.
    fn records_to_batch(&self, records: &[NodeRecord]) -> Result<RecordBatch> {
        let id_array = StringArray::from(
            records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        );
        let name_array = StringArray::from(
            records.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        );
        let content_array = StringArray::from(
            records
                .iter()
                .map(|r| r.content.as_str())
                .collect::<Vec<_>>(),
        );
        let file_path_array = StringArray::from(
            records
                .iter()
                .map(|r| r.file_path.as_str())
                .collect::<Vec<_>>(),
        );
        let node_type_array = StringArray::from(
            records
                .iter()
                .map(|r| r.node_type.as_str())
                .collect::<Vec<_>>(),
        );

        let all_floats: Vec<f32> = records.iter().flat_map(|r| r.embedding.iter().copied()).collect();
        let values = Float32Array::from(all_floats);
        let field = Arc::new(Field::new("item", DataType::Float32, true));
        let vector_array = FixedSizeListArray::new(field, self.dims as i32, Arc::new(values), None);

        let batch = RecordBatch::try_new(
            self.schema(),
            vec![
                Arc::new(id_array),
                Arc::new(name_array),
                Arc::new(content_array),
                Arc::new(file_path_array),
                Arc::new(node_type_array),
                Arc::new(vector_array),
            ],
        )?;

        Ok(batch)
    }

    /// Upsert node records into the store. Deletes existing records with same IDs first.
    pub async fn upsert(&self, records: &[NodeRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let table = self.ensure_table().await?;

        // Delete existing records with these IDs
        let id_list = records
            .iter()
            .map(|r| format!("'{}'", r.id))
            .collect::<Vec<_>>()
            .join(", ");
        let filter = format!("id IN ({})", id_list);
        let _ = table.delete(&filter).await;

        let batch = self.records_to_batch(records)?;
        let schema = self.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table
            .add(Box::new(batches))
            .execute()
            .await
            .context("failed to add records to LanceDB")?;

        Ok(())
    }

    /// Delete records by node IDs.
    pub async fn delete(&self, ids: &[NodeId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let table = self.ensure_table().await?;
        let id_list = ids
            .iter()
            .map(|id| format!("'{}'", id))
            .collect::<Vec<_>>()
            .join(", ");
        let filter = format!("id IN ({})", id_list);
        table.delete(&filter).await?;
        Ok(())
    }

    /// Vector similarity search. Returns top-k results.
    pub async fn search(&self, query_embedding: &[f32], top_k: usize) -> Result<Vec<SearchResult>> {
        let table = self.ensure_table().await?;

        let mut stream = table
            .vector_search(query_embedding)?
            .limit(top_k)
            .execute()
            .await
            .context("vector search failed")?;

        let mut search_results = Vec::new();
        while let Some(batch) = stream.try_next().await? {
            let id_col = batch
                .column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let content_col = batch
                .column_by_name("content")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let score_col = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            if let (Some(ids), Some(contents), Some(scores)) = (id_col, content_col, score_col) {
                for i in 0..ids.len() {
                    search_results.push(SearchResult {
                        id: ids.value(i).to_string(),
                        content: contents.value(i).to_string(),
                        score: 1.0 - scores.value(i),
                    });
                }
            }
        }

        Ok(search_results)
    }

    /// Get content for a specific node by ID.
    pub async fn get_content(&self, id: &NodeId) -> Result<Option<String>> {
        let table = self.ensure_table().await?;
        let filter = format!("id = '{}'", id);

        let mut stream = table
            .query()
            .only_if(filter)
            .execute()
            .await?;

        while let Some(batch) = stream.try_next().await? {
            let content_col = batch
                .column_by_name("content")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            if let Some(contents) = content_col {
                if contents.len() > 0 {
                    return Ok(Some(contents.value(0).to_string()));
                }
            }
        }

        Ok(None)
    }

    /// Batch get content for multiple node IDs in a single query.
    pub async fn get_contents_batch(
        &self,
        ids: &[NodeId],
    ) -> Result<HashMap<NodeId, String>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        let table = self.ensure_table().await?;
        let id_list = ids
            .iter()
            .map(|id| format!("'{}'", id))
            .collect::<Vec<_>>()
            .join(", ");
        let filter = format!("id IN ({})", id_list);

        let mut stream = table
            .query()
            .only_if(filter)
            .execute()
            .await?;

        let mut results = HashMap::new();
        while let Some(batch) = stream.try_next().await? {
            let id_col = batch
                .column_by_name("id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let content_col = batch
                .column_by_name("content")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            if let (Some(ids_arr), Some(contents)) = (id_col, content_col) {
                for i in 0..ids_arr.len() {
                    if let Ok(uuid) = ids_arr.value(i).parse::<uuid::Uuid>() {
                        results.insert(uuid, contents.value(i).to_string());
                    }
                }
            }
        }

        Ok(results)
    }

    /// Get total record count.
    pub async fn count(&self) -> Result<usize> {
        let table = self.ensure_table().await?;
        let count = table.count_rows(None).await?;
        Ok(count)
    }
}
