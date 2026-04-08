//! `LinfaClusterer` — concrete `Clusterer` impl using `linfa-clustering`
//! mini-batch k-means. Lives in infra so the core crate stays free of
//! numerical dependencies.

use linfa::DatasetBase;
use linfa::traits::{Fit, Predict};
use linfa_clustering::KMeans;
use ndarray::{Array1, Array2};
use rand::SeedableRng;
use rand::rngs::StdRng;

use kenjaku_core::error::{Error, Result};
use kenjaku_core::traits::clusterer::{Cluster, Clusterer};

/// Deterministic k-means clusterer. Seed is fixed (`0xC0FFEE`) so two
/// runs over the same input produce identical clusters — important for
/// regression tests and for fingerprint-driven skip logic in the worker.
pub struct LinfaClusterer {
    seed: u64,
    max_iter: u64,
    tolerance: f64,
}

impl Default for LinfaClusterer {
    fn default() -> Self {
        Self {
            seed: 0x00C0_FFEE,
            max_iter: 100,
            tolerance: 1e-4,
        }
    }
}

impl LinfaClusterer {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Clusterer for LinfaClusterer {
    fn kmeans(&self, vectors: &[Vec<f32>], k: usize) -> Result<Vec<Cluster>> {
        if vectors.is_empty() {
            return Ok(Vec::new());
        }

        let n = vectors.len();
        let dim = vectors[0].len();
        if dim == 0 {
            return Err(Error::Internal(
                "LinfaClusterer: input vectors are zero-dimensional".to_string(),
            ));
        }
        for (i, v) in vectors.iter().enumerate() {
            if v.len() != dim {
                return Err(Error::Internal(format!(
                    "LinfaClusterer: vector {i} has length {} but expected {dim}",
                    v.len()
                )));
            }
        }

        // Spec §4.4: "fewer than K*3 samples → reduce K".
        let effective_k = k.max(1).min(n).min((n / 3).max(1));

        // Build an (n × dim) f64 matrix; linfa wants f64.
        let mut flat = Vec::with_capacity(n * dim);
        for v in vectors {
            for &x in v {
                flat.push(x as f64);
            }
        }
        let data = Array2::from_shape_vec((n, dim), flat).map_err(|e| {
            Error::Internal(format!("LinfaClusterer: failed to build ndarray: {e}"))
        })?;
        let targets: Array1<usize> = Array1::zeros(n);
        let dataset = DatasetBase::new(data, targets);

        let rng = StdRng::seed_from_u64(self.seed);
        let model = KMeans::params_with_rng(effective_k, rng)
            .max_n_iterations(self.max_iter)
            .tolerance(self.tolerance)
            .fit(&dataset)
            .map_err(|e| Error::Internal(format!("LinfaClusterer: KMeans fit failed: {e}")))?;

        let assignments = model.predict(&dataset);
        let centroids = model.centroids().clone();

        let mut clusters: Vec<Cluster> = (0..effective_k)
            .map(|id| Cluster {
                id,
                member_indices: Vec::new(),
                centroid: centroids
                    .row(id)
                    .iter()
                    .map(|&x| x as f32)
                    .collect::<Vec<f32>>(),
            })
            .collect();

        for (idx, cluster_id) in assignments.iter().enumerate() {
            if let Some(c) = clusters.get_mut(*cluster_id) {
                c.member_indices.push(idx);
            }
        }

        // Drop empty clusters — linfa can produce them if k > unique
        // points. The trait contract says we may return fewer than k.
        clusters.retain(|c| !c.member_indices.is_empty());
        // Re-id densely after dropping.
        for (new_id, c) in clusters.iter_mut().enumerate() {
            c.id = new_id;
        }

        Ok(clusters)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three_obvious_clusters() -> Vec<Vec<f32>> {
        // Three tight 2D clusters around (0,0), (10,10), (-10,10).
        let mut v = Vec::new();
        for i in 0..6 {
            v.push(vec![0.0 + (i as f32) * 0.01, 0.0 + (i as f32) * 0.01]);
        }
        for i in 0..6 {
            v.push(vec![10.0 + (i as f32) * 0.01, 10.0 + (i as f32) * 0.01]);
        }
        for i in 0..6 {
            v.push(vec![-10.0 + (i as f32) * 0.01, 10.0 + (i as f32) * 0.01]);
        }
        v
    }

    #[test]
    fn kmeans_finds_three_clusters_on_obvious_fixture() {
        let clusterer = LinfaClusterer::new();
        let vectors = three_obvious_clusters();
        let clusters = clusterer.kmeans(&vectors, 3).unwrap();
        assert_eq!(clusters.len(), 3);
        // Every point should be assigned exactly once.
        let total: usize = clusters.iter().map(|c| c.member_indices.len()).sum();
        assert_eq!(total, vectors.len());
        // No cluster should be empty (we drop empty ones).
        for c in &clusters {
            assert!(!c.member_indices.is_empty());
            assert_eq!(c.centroid.len(), 2);
        }
    }

    #[test]
    fn kmeans_empty_input_returns_empty() {
        let clusterer = LinfaClusterer::new();
        let clusters = clusterer.kmeans(&[], 5).unwrap();
        assert!(clusters.is_empty());
    }

    #[test]
    fn kmeans_reduces_k_for_small_inputs() {
        let clusterer = LinfaClusterer::new();
        // 4 points but asking for k=20 → spec says reduce to floor(n/3) = 1.
        let vectors = vec![
            vec![0.0, 0.0],
            vec![0.1, 0.1],
            vec![0.2, 0.2],
            vec![0.3, 0.3],
        ];
        let clusters = clusterer.kmeans(&vectors, 20).unwrap();
        assert!(clusters.len() <= 4);
        assert!(!clusters.is_empty());
    }

    #[test]
    fn kmeans_rejects_mismatched_dims() {
        let clusterer = LinfaClusterer::new();
        let vectors = vec![vec![1.0, 2.0], vec![3.0, 4.0, 5.0]];
        assert!(clusterer.kmeans(&vectors, 2).is_err());
    }

    #[test]
    fn kmeans_is_deterministic_across_runs() {
        let vectors = three_obvious_clusters();
        let a = LinfaClusterer::new().kmeans(&vectors, 3).unwrap();
        let b = LinfaClusterer::new().kmeans(&vectors, 3).unwrap();
        // Same fixed seed → identical centroids.
        assert_eq!(a.len(), b.len());
        for (ca, cb) in a.iter().zip(b.iter()) {
            assert_eq!(ca.member_indices, cb.member_indices);
        }
    }
}
