use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

use unly_core::{Result, model::EmbeddingRequest, provider::Provider, types::Timestamp};
use unly_db::{
    Database,
    repo::memory::{MemoryEntryRow, MemoryRepo},
};

use crate::{
    scope::MemoryScope,
    similarity::{cosine_similarity, deserialize_embedding, serialize_embedding},
};

/// A single memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub scope: MemoryScope,
    pub content: String,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
}

/// A query for semantic memory retrieval.
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub scope: MemoryScope,
    pub query: String,
    pub top_k: usize,
    pub similarity_threshold: f32,
}

/// A scored memory retrieval result.
#[derive(Debug, Clone)]
pub struct MemoryResult {
    pub entry: MemoryEntry,
    pub similarity: f32,
}

/// The memory store: handles ingestion, embedding, storage, and retrieval.
pub struct MemoryStore {
    db: Database,
    embedding_provider: Arc<dyn Provider>,
    embedding_model: String,
    #[allow(dead_code)]
    top_k: usize,
    #[allow(dead_code)]
    similarity_threshold: f32,
}

impl MemoryStore {
    pub fn new(
        db: Database,
        embedding_provider: Arc<dyn Provider>,
        embedding_model: impl Into<String>,
        top_k: usize,
        similarity_threshold: f32,
    ) -> Self {
        Self {
            db,
            embedding_provider,
            embedding_model: embedding_model.into(),
            top_k,
            similarity_threshold,
        }
    }

    /// Store a new memory entry. Generates an embedding and persists to SQLite.
    pub async fn store(
        &self,
        scope: MemoryScope,
        content: impl Into<String>,
        source_type: Option<String>,
        source_id: Option<String>,
        metadata: serde_json::Value,
        expires_at: Option<Timestamp>,
    ) -> Result<MemoryEntry> {
        let content = content.into();
        let id = Uuid::new_v4().to_string();

        // Generate embedding.
        let embedding_resp = self
            .embedding_provider
            .embeddings(EmbeddingRequest {
                model: self.embedding_model.clone(),
                input: vec![content.clone()],
            })
            .await?;

        let embedding_vec = embedding_resp
            .embeddings
            .into_iter()
            .next()
            .ok_or_else(|| unly_core::Error::Memory("no embedding returned".to_string()))?;

        let embedding_bytes = serialize_embedding(&embedding_vec);

        let now = Utc::now();
        let row = MemoryEntryRow {
            id: id.clone(),
            scope_type: scope.scope_type().to_string(),
            scope_id: scope.scope_id().to_string(),
            content: content.clone(),
            embedding: embedding_bytes,
            source_type: source_type.clone(),
            source_id: source_id.clone(),
            metadata: serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string()),
            created_at: now,
            expires_at,
        };

        let repo = MemoryRepo::new(self.db.conn());
        repo.insert(&row)
            .await
            .map_err(|e| unly_core::Error::Memory(e.to_string()))?;

        debug!(scope_type = %scope.scope_type(), scope_id = %scope.scope_id(), "stored memory entry");

        Ok(MemoryEntry {
            id,
            scope,
            content,
            source_type,
            source_id,
            metadata,
            created_at: now,
            expires_at,
        })
    }

    /// Retrieve the top-k most semantically similar memories for a query.
    pub async fn retrieve(&self, query: MemoryQuery) -> Result<Vec<MemoryResult>> {
        // Generate embedding for the query.
        let embedding_resp = self
            .embedding_provider
            .embeddings(EmbeddingRequest {
                model: self.embedding_model.clone(),
                input: vec![query.query.clone()],
            })
            .await?;

        let query_vec = embedding_resp
            .embeddings
            .into_iter()
            .next()
            .ok_or_else(|| {
                unly_core::Error::Memory("no embedding returned for query".to_string())
            })?;

        // Load all entries for this scope.
        let repo = MemoryRepo::new(self.db.conn());
        let rows = repo
            .list_by_scope(query.scope.scope_type(), query.scope.scope_id())
            .await
            .map_err(|e| unly_core::Error::Memory(e.to_string()))?;

        let threshold = query.similarity_threshold;
        let top_k = query.top_k;

        // Compute similarities.
        let mut scored: Vec<(f32, MemoryEntry)> = rows
            .into_iter()
            .filter_map(|row| {
                let embedding = deserialize_embedding(&row.embedding);
                let score = cosine_similarity(&query_vec, &embedding);
                if score < threshold {
                    return None;
                }
                let scope = match row.scope_type.as_str() {
                    "user" => MemoryScope::User(row.scope_id.clone()),
                    "chat" => MemoryScope::Chat(row.scope_id.clone()),
                    "agent" => MemoryScope::Agent(row.scope_id.clone()),
                    _ => MemoryScope::Subagent(row.scope_id.clone()),
                };
                let metadata: serde_json::Value =
                    serde_json::from_str(&row.metadata).unwrap_or_default();
                Some((
                    score,
                    MemoryEntry {
                        id: row.id,
                        scope,
                        content: row.content,
                        source_type: row.source_type,
                        source_id: row.source_id,
                        metadata,
                        created_at: row.created_at,
                        expires_at: row.expires_at,
                    },
                ))
            })
            .collect();

        // Sort by similarity descending.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        Ok(scored
            .into_iter()
            .map(|(similarity, entry)| MemoryResult { entry, similarity })
            .collect())
    }

    /// Delete expired memory entries.
    pub async fn prune_expired(&self) -> Result<u64> {
        let repo = MemoryRepo::new(self.db.conn());
        repo.delete_expired()
            .await
            .map_err(|e| unly_core::Error::Memory(e.to_string()))
    }

    /// Delete a memory entry by ID.
    pub async fn delete(&self, id: &str) -> Result<bool> {
        let repo = MemoryRepo::new(self.db.conn());
        repo.delete_by_id(id)
            .await
            .map_err(|e| unly_core::Error::Memory(e.to_string()))
    }

    /// Count entries for a scope.
    pub async fn count(&self, scope: &MemoryScope) -> Result<u64> {
        let repo = MemoryRepo::new(self.db.conn());
        repo.count_by_scope(scope.scope_type(), scope.scope_id())
            .await
            .map_err(|e| unly_core::Error::Memory(e.to_string()))
    }
}
