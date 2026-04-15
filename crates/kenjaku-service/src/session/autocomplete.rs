use std::collections::HashSet;

use tracing::instrument;

use kenjaku_core::error::Result;
use kenjaku_core::types::tenant::TenantContext;
use kenjaku_infra::postgres::TrendingRepository;
use kenjaku_infra::qdrant::QdrantClient;

use crate::quality::prettify_title;

/// Service for autocomplete suggestions combining popular queries and document titles.
#[derive(Clone)]
pub struct AutocompleteService {
    trending_repo: TrendingRepository,
    qdrant: QdrantClient,
    /// Crowdsourcing quality floor — popular queries with `search_count`
    /// below this are filtered out at the SQL level.
    min_count: i64,
}

impl AutocompleteService {
    pub fn new(trending_repo: TrendingRepository, qdrant: QdrantClient, min_count: i64) -> Self {
        Self {
            trending_repo,
            qdrant,
            min_count,
        }
    }

    /// Get autocomplete suggestions for a partial query.
    ///
    /// Phase 3b: takes `tctx` so the popular-queries read is
    /// tenant-scoped. Not currently called from any handler (autocomplete
    /// + top-searches go through `SuggestionService`), but the signature
    /// is kept in lock-step with the repo so slice 3c can drop it into
    /// a per-tenant extractor without reshaping this call.
    #[instrument(skip(self, tctx), fields(
        tenant_id = %tctx.tenant_id.as_str(),
        plan_tier = ?tctx.plan_tier,
    ))]
    pub async fn suggest(
        &self,
        tctx: &TenantContext,
        query: &str,
        locale: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        let mut suggestions = HashSet::new();

        // Source 1: Popular past searches matching prefix (with quality floor)
        let popular = self
            .trending_repo
            .search_popular(
                tctx.tenant_id.as_str(),
                locale,
                query,
                limit,
                self.min_count,
            )
            .await?;

        for pq in popular {
            suggestions.insert(pq.query);
        }

        // Source 2: Qdrant document title similarities
        if suggestions.len() < limit {
            let remaining = limit - suggestions.len();
            // Over-fetch from Qdrant since prettify + dedupe may collapse
            // multiple slug variants onto the same display title.
            let titles = self.qdrant.search_titles(query, remaining * 2).await?;
            for title in titles {
                let pretty = prettify_title(&title);
                if !pretty.is_empty() {
                    suggestions.insert(pretty);
                }
            }
        }

        let mut result: Vec<String> = suggestions.into_iter().collect();
        result.truncate(limit);
        Ok(result)
    }
}
