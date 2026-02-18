use pgvector::Vector;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::embedding::FeedbackEmbedding;

#[derive(Clone)]
pub struct EmbeddingDb {
    pool: PgPool,
}

impl EmbeddingDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store a feedback chunk with its embedding vector.
    pub async fn store_chunk(
        &self,
        feedback_id: Uuid,
        chunk_index: i32,
        chunk_text: &str,
        embedding: &[f32],
    ) -> Result<FeedbackEmbedding, sqlx::Error> {
        let vec = Vector::from(embedding.to_vec());
        let row = sqlx::query_as::<_, FeedbackEmbedding>(
            r#"
            INSERT INTO feedback_embeddings (feedback_id, chunk_index, chunk_text, embedding)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (feedback_id, chunk_index) DO UPDATE
                SET chunk_text = EXCLUDED.chunk_text, embedding = EXCLUDED.embedding
            RETURNING id, feedback_id, chunk_index, chunk_text, created_at
            "#,
        )
        .bind(feedback_id)
        .bind(chunk_index)
        .bind(chunk_text)
        .bind(vec)
        .fetch_one(&self.pool)
        .await?;
        Ok(row)
    }

    /// Semantic search across feedback chunks using cosine similarity.
    /// Returns the most relevant chunks with their feedback_id and similarity score.
    pub async fn search_similar(
        &self,
        query_embedding: &[f32],
        limit: i64,
    ) -> Result<Vec<SimilarChunk>, sqlx::Error> {
        let vec = Vector::from(query_embedding.to_vec());
        let rows = sqlx::query_as::<_, SimilarChunk>(
            r#"
            SELECT
                fe.id, fe.feedback_id, fe.chunk_index, fe.chunk_text,
                1 - (fe.embedding <=> $1) AS similarity,
                f.feedback_type, f.subject AS feedback_subject, f.rating
            FROM feedback_embeddings fe
            JOIN feedback f ON f.id = fe.feedback_id
            WHERE fe.embedding IS NOT NULL
            ORDER BY fe.embedding <=> $1
            LIMIT $2
            "#,
        )
        .bind(vec)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Get all chunks for a specific feedback entry.
    pub async fn get_chunks(
        &self,
        feedback_id: Uuid,
    ) -> Result<Vec<FeedbackEmbedding>, sqlx::Error> {
        let rows = sqlx::query_as::<_, FeedbackEmbedding>(
            r#"
            SELECT id, feedback_id, chunk_index, chunk_text, created_at
            FROM feedback_embeddings
            WHERE feedback_id = $1
            ORDER BY chunk_index
            "#,
        )
        .bind(feedback_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct SimilarChunk {
    pub id: Uuid,
    pub feedback_id: Uuid,
    pub chunk_index: i32,
    pub chunk_text: String,
    pub similarity: f64,
    pub feedback_type: String,
    pub feedback_subject: Option<String>,
    pub rating: Option<i32>,
}
