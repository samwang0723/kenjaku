//! Clustering abstraction used by the `SuggestionRefreshWorker`.
//!
//! The core crate defines the trait and the data shapes. Concrete
//! implementations live in `kenjaku-infra` (e.g. `LinfaClusterer` on top
//! of `linfa-clustering` mini-batch k-means). This keeps `kenjaku-core`
//! free of external numerical dependencies.

use crate::error::Result;

/// A single cluster produced by a `Clusterer::kmeans` call.
///
/// `id` is a dense index in `0..k`. `member_indices` point back into
/// the input vector slice; the worker uses them to pick the top-N
/// chunks closest to the centroid when building the LLM prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub id: usize,
    pub member_indices: Vec<usize>,
    pub centroid: Vec<f32>,
}

/// Abstraction over a clustering algorithm. The worker only needs
/// k-means today; if other algorithms are added later we extend the
/// trait rather than forking it.
pub trait Clusterer: Send + Sync {
    /// Cluster `vectors` into at most `k` groups. Implementations MAY
    /// reduce `k` internally when there are too few samples to form
    /// meaningful clusters (spec §4.4 says "fewer than K*3 samples →
    /// reduce K"). The returned `Vec<Cluster>` is non-empty as long as
    /// `vectors` is non-empty.
    fn kmeans(&self, vectors: &[Vec<f32>], k: usize) -> Result<Vec<Cluster>>;
}
