use std::collections::HashSet;

use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_infra::postgres::TrendingRepository;
use kenjaku_infra::qdrant::QdrantClient;

/// Service for autocomplete suggestions combining popular queries and document titles.
#[derive(Clone)]
pub struct AutocompleteService {
    trending_repo: TrendingRepository,
    qdrant: QdrantClient,
}

impl AutocompleteService {
    pub fn new(trending_repo: TrendingRepository, qdrant: QdrantClient) -> Self {
        Self {
            trending_repo,
            qdrant,
        }
    }

    /// Get autocomplete suggestions for a partial query.
    #[instrument(skip(self))]
    pub async fn suggest(&self, query: &str, locale: &str, limit: usize) -> Result<Vec<String>> {
        let mut suggestions = HashSet::new();

        // Source 1: Popular past searches matching prefix
        let popular = self
            .trending_repo
            .search_popular(locale, query, limit)
            .await?;

        for pq in popular {
            suggestions.insert(pq.query);
        }

        // Source 2: Qdrant document title similarities
        if suggestions.len() < limit {
            let remaining = limit - suggestions.len();
            let titles = self.qdrant.search_titles(query, remaining).await?;
            for title in titles {
                suggestions.insert(title);
            }
        }

        let mut result: Vec<String> = suggestions.into_iter().collect();
        result.truncate(limit);
        Ok(result)
    }
}
