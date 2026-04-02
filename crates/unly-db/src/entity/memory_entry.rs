use sea_orm::entity::prelude::*;

/// SeaORM entity for the `memory_entries` table.
///
/// Embeddings are stored as BLOB (little-endian IEEE 754 f32 sequence).
/// Cosine similarity search is performed in Rust after fetching candidates.
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "memory_entries")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// Scope type: `user`, `chat`, `agent`, or `subagent`.
    pub scope_type: String,
    pub scope_id: String,
    pub content: String,
    /// Serialized embedding vector (little-endian f32 slice).
    #[sea_orm(column_type = "Blob")]
    pub embedding: Vec<u8>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    /// JSON metadata object.
    pub metadata: String,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
