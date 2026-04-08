use qdrant_client::Qdrant;
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, CreateFieldIndexCollectionBuilder, Distance, FieldType,
    Filter, PointStruct, ScrollPointsBuilder, SearchPointsBuilder, TextIndexParamsBuilder,
    TokenizerType, UpsertPointsBuilder, Value, VectorParamsBuilder,
    point_id::PointIdOptions, vectors_output::VectorsOptions,
};

/// Lightweight projection of a Qdrant point returned by
/// [`QdrantClient::scroll_with_vectors`] for the suggestion refresh
/// worker. Only the fields the clusterer needs are surfaced.
#[derive(Debug, Clone)]
pub struct ScrolledPoint {
    pub id: String,
    pub vector: Vec<f32>,
    pub text: String,
}
use tracing::info;

use kenjaku_core::config::QdrantConfig;
use kenjaku_core::error::{Error, Result};
use kenjaku_core::types::search::{RetrievalMethod, RetrievedChunk};

/// Wrapper around the Qdrant client with collection management.
#[derive(Clone)]
pub struct QdrantClient {
    client: Qdrant,
    config: QdrantConfig,
}

impl QdrantClient {
    /// Create a new Qdrant client.
    pub async fn new(config: QdrantConfig) -> Result<Self> {
        let client = Qdrant::from_url(&config.url)
            .build()
            .map_err(|e| Error::VectorStore(format!("Failed to connect to Qdrant: {e}")))?;

        Ok(Self { client, config })
    }

    /// Ensure the collection exists with proper schema.
    pub async fn ensure_collection(&self) -> Result<()> {
        let collections = self
            .client
            .list_collections()
            .await
            .map_err(|e| Error::VectorStore(e.to_string()))?;

        let exists = collections
            .collections
            .iter()
            .any(|c| c.name == self.config.collection_name);

        if !exists {
            info!(
                collection = %self.config.collection_name,
                vector_size = self.config.vector_size,
                "Creating Qdrant collection"
            );

            self.client
                .create_collection(
                    CreateCollectionBuilder::new(&self.config.collection_name).vectors_config(
                        VectorParamsBuilder::new(self.config.vector_size, Distance::Cosine),
                    ),
                )
                .await
                .map_err(|e| Error::VectorStore(format!("Failed to create collection: {e}")))?;

            // Create full-text index on contextualized_content for BM25-style search
            self.create_text_index("contextualized_content").await?;
            self.create_text_index("title").await?;

            info!(
                collection = %self.config.collection_name,
                "Collection created with text indices"
            );
        }

        Ok(())
    }

    /// Create a full-text index on a payload field.
    async fn create_text_index(&self, field_name: &str) -> Result<()> {
        self.client
            .create_field_index(
                CreateFieldIndexCollectionBuilder::new(
                    &self.config.collection_name,
                    field_name,
                    FieldType::Text,
                )
                .field_index_params(
                    TextIndexParamsBuilder::new(TokenizerType::Word)
                        .min_token_len(2u64)
                        .max_token_len(20u64)
                        .lowercase(true),
                ),
            )
            .await
            .map_err(|e| {
                Error::VectorStore(format!("Failed to create text index on {field_name}: {e}"))
            })?;

        Ok(())
    }

    /// Upsert points (chunks with embeddings) into the collection.
    pub async fn upsert_points(&self, points: Vec<PointData>) -> Result<()> {
        let qdrant_points: Vec<PointStruct> = points
            .into_iter()
            .map(|p| {
                let mut payload = std::collections::HashMap::new();
                payload.insert("doc_id".to_string(), Value::from(p.doc_id.clone()));
                payload.insert("chunk_id".to_string(), Value::from(p.chunk_id.clone()));
                payload.insert("title".to_string(), Value::from(p.title.clone()));
                payload.insert(
                    "original_content".to_string(),
                    Value::from(p.original_content.clone()),
                );
                payload.insert(
                    "contextualized_content".to_string(),
                    Value::from(p.contextualized_content.clone()),
                );
                if let Some(ref url) = p.source_url {
                    payload.insert("source_url".to_string(), Value::from(url.clone()));
                }
                payload.insert("doc_type".to_string(), Value::from(p.doc_type.clone()));
                payload.insert(
                    "ingested_at".to_string(),
                    Value::from(p.ingested_at.clone()),
                );

                PointStruct::new(p.point_id.clone(), p.embedding.clone(), payload)
            })
            .collect();

        self.client
            .upsert_points(
                UpsertPointsBuilder::new(&self.config.collection_name, qdrant_points).wait(true),
            )
            .await
            .map_err(|e| Error::VectorStore(format!("Failed to upsert points: {e}")))?;

        Ok(())
    }

    /// Vector similarity search.
    pub async fn vector_search(
        &self,
        embedding: Vec<f32>,
        top_k: usize,
    ) -> Result<Vec<RetrievedChunk>> {
        let results = self
            .client
            .search_points(
                SearchPointsBuilder::new(&self.config.collection_name, embedding, top_k as u64)
                    .with_payload(true),
            )
            .await
            .map_err(|e| Error::VectorStore(format!("Vector search failed: {e}")))?;

        let chunks = results
            .result
            .into_iter()
            .filter_map(|point| {
                let payload = point.payload;
                Some(RetrievedChunk {
                    doc_id: extract_string(&payload, "doc_id")?,
                    chunk_id: extract_string(&payload, "chunk_id")?,
                    title: extract_string(&payload, "title").unwrap_or_default(),
                    original_content: extract_string(&payload, "original_content")
                        .unwrap_or_default(),
                    contextualized_content: extract_string(&payload, "contextualized_content")
                        .unwrap_or_default(),
                    source_url: extract_string(&payload, "source_url"),
                    score: point.score,
                    retrieval_method: RetrievalMethod::Vector,
                })
            })
            .collect();

        Ok(chunks)
    }

    /// Full-text search on contextualized_content (BM25-style via Qdrant text index).
    pub async fn fulltext_search(&self, query: &str, top_k: usize) -> Result<Vec<RetrievedChunk>> {
        // Use scroll with filter for text matching
        // Qdrant's text index supports full-text search via match conditions
        let filter = Filter::must([Condition::matches_text("contextualized_content", query)]);

        let results = self
            .client
            .search_points(
                SearchPointsBuilder::new(
                    &self.config.collection_name,
                    // Zero vector as placeholder -- score comes from text matching
                    vec![0.0; self.config.vector_size as usize],
                    top_k as u64,
                )
                .filter(filter)
                .with_payload(true),
            )
            .await
            .map_err(|e| Error::VectorStore(format!("Full-text search failed: {e}")))?;

        let chunks = results
            .result
            .into_iter()
            .filter_map(|point| {
                let payload = point.payload;
                Some(RetrievedChunk {
                    doc_id: extract_string(&payload, "doc_id")?,
                    chunk_id: extract_string(&payload, "chunk_id")?,
                    title: extract_string(&payload, "title").unwrap_or_default(),
                    original_content: extract_string(&payload, "original_content")
                        .unwrap_or_default(),
                    contextualized_content: extract_string(&payload, "contextualized_content")
                        .unwrap_or_default(),
                    source_url: extract_string(&payload, "source_url"),
                    score: point.score,
                    retrieval_method: RetrievalMethod::FullText,
                })
            })
            .collect();

        Ok(chunks)
    }

    /// Search document titles for autocomplete suggestions.
    pub async fn search_titles(&self, query: &str, limit: usize) -> Result<Vec<String>> {
        let filter = Filter::must([Condition::matches_text("title", query)]);

        let results = self
            .client
            .search_points(
                SearchPointsBuilder::new(
                    &self.config.collection_name,
                    vec![0.0; self.config.vector_size as usize],
                    limit as u64,
                )
                .filter(filter)
                .with_payload(true),
            )
            .await
            .map_err(|e| Error::VectorStore(format!("Title search failed: {e}")))?;

        let titles: Vec<String> = results
            .result
            .into_iter()
            .filter_map(|point| extract_string(&point.payload, "title"))
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        Ok(titles)
    }

    /// Return the approximate `points_count` of the configured
    /// collection. Used by the suggestion refresh worker to build a
    /// fingerprint. Returns 0 when Qdrant reports no count.
    pub async fn collection_info(&self, collection_name: &str) -> Result<u64> {
        let response = self
            .client
            .collection_info(collection_name.to_string())
            .await
            .map_err(|e| Error::VectorStore(format!("collection_info failed: {e}")))?;
        Ok(response
            .result
            .and_then(|info| info.points_count)
            .unwrap_or(0))
    }

    /// Scroll up to `limit` points returning their id, dense vector,
    /// and contextualized text. Used by the suggestion refresh worker
    /// to build clusters over the whole corpus.
    pub async fn scroll_with_vectors(
        &self,
        collection_name: &str,
        limit: u32,
    ) -> Result<Vec<ScrolledPoint>> {
        let response = self
            .client
            .scroll(
                ScrollPointsBuilder::new(collection_name.to_string())
                    .limit(limit)
                    .with_payload(true)
                    .with_vectors(true),
            )
            .await
            .map_err(|e| Error::VectorStore(format!("scroll failed: {e}")))?;

        let mut out = Vec::with_capacity(response.result.len());
        for point in response.result {
            let id = match point.id.and_then(|p| p.point_id_options) {
                Some(PointIdOptions::Num(n)) => n.to_string(),
                Some(PointIdOptions::Uuid(u)) => u,
                None => continue,
            };

            let vector: Vec<f32> = match point.vectors.and_then(|v| v.vectors_options) {
                Some(VectorsOptions::Vector(v)) => {
                    #[allow(deprecated)]
                    {
                        v.data
                    }
                }
                // Named/multi vectors are not used by kenjaku's collection.
                _ => continue,
            };

            if vector.is_empty() {
                continue;
            }

            let text = extract_string(&point.payload, "contextualized_content")
                .or_else(|| extract_string(&point.payload, "original_content"))
                .or_else(|| extract_string(&point.payload, "title"))
                .unwrap_or_default();

            out.push(ScrolledPoint { id, vector, text });
        }
        Ok(out)
    }

    /// Check if Qdrant is healthy.
    pub async fn health_check(&self) -> Result<()> {
        self.client
            .health_check()
            .await
            .map_err(|e| Error::VectorStore(format!("Qdrant health check failed: {e}")))?;
        Ok(())
    }
}

/// Data for a single point to upsert.
pub struct PointData {
    pub point_id: String,
    pub doc_id: String,
    pub chunk_id: String,
    pub title: String,
    pub original_content: String,
    pub contextualized_content: String,
    pub source_url: Option<String>,
    pub doc_type: String,
    pub ingested_at: String,
    pub embedding: Vec<f32>,
}

/// Extract a string value from a Qdrant payload.
fn extract_string(payload: &std::collections::HashMap<String, Value>, key: &str) -> Option<String> {
    payload.get(key).and_then(|v| {
        v.kind.as_ref().and_then(|k| {
            if let qdrant_client::qdrant::value::Kind::StringValue(s) = k {
                Some(s.clone())
            } else {
                None
            }
        })
    })
}
