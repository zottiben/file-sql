use std::path::PathBuf;
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::{Error, Result};

/// Produces dense embeddings for chunk text and search queries. Kept behind a
/// trait so indexing/search never depend on a specific model or runtime (and so
/// tests can substitute a deterministic offline embedder).
pub trait Embedder: Send + Sync {
    fn dims(&self) -> usize;
    /// Embed a batch of texts, returning one vector per input in order.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Convenience for embedding a single query string.
    fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut out = self.embed(&[text])?;
        out.pop()
            .ok_or_else(|| Error::Embedding("embedder returned no vectors".into()))
    }
}

/// Local ONNX embedder (fastembed). The model is downloaded once into the cache
/// dir and then loaded from disk; no API keys, so the tool stays model-agnostic.
pub struct FastEmbedder {
    inner: Mutex<TextEmbedding>,
    dims: usize,
}

impl FastEmbedder {
    pub fn new(model_name: &str, cache_dir: PathBuf) -> Result<Self> {
        let (model, dims) = resolve_model(model_name);
        let options = InitOptions::new(model)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(true);
        let inner =
            TextEmbedding::try_new(options).map_err(|e| Error::Embedding(format!("{e:#}")))?;
        Ok(FastEmbedder {
            inner: Mutex::new(inner),
            dims,
        })
    }

    /// Shared per-user cache directory so the model is downloaded once across
    /// repos, not once per repo.
    pub fn default_cache_dir() -> PathBuf {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(".cache/file-sql/fastembed"))
            .unwrap_or_else(|| PathBuf::from(".fastembed_cache"))
    }
}

impl Embedder for FastEmbedder {
    fn dims(&self) -> usize {
        self.dims
    }

    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut model = self
            .inner
            .lock()
            .map_err(|e| Error::Embedding(format!("embedder mutex poisoned: {e}")))?;
        model
            .embed(texts, None)
            .map_err(|e| Error::Embedding(e.to_string()))
    }
}

/// Map a friendly/HuggingFace-style model name to a fastembed model + its fixed
/// dimensionality. Unknown names fall back to bge-small so the tool still runs.
fn resolve_model(name: &str) -> (EmbeddingModel, usize) {
    let key = name.to_ascii_lowercase();
    let pick = |needle: &str| key.contains(needle);
    if pick("bge-small") || pick("bgesmall") {
        (EmbeddingModel::BGESmallENV15, 384)
    } else if pick("minilm-l6") || pick("all-minilm-l6") {
        (EmbeddingModel::AllMiniLML6V2, 384)
    } else if pick("bge-base") {
        (EmbeddingModel::BGEBaseENV15, 768)
    } else if pick("bge-large") {
        (EmbeddingModel::BGELargeENV15, 1024)
    } else {
        (EmbeddingModel::BGESmallENV15, 384)
    }
}

#[cfg(test)]
pub(crate) struct HashEmbedder {
    dims: usize,
}

#[cfg(test)]
impl HashEmbedder {
    pub(crate) fn new(dims: usize) -> Self {
        HashEmbedder { dims }
    }
}

#[cfg(test)]
impl Embedder for HashEmbedder {
    fn dims(&self) -> usize {
        self.dims
    }

    /// Deterministic bag-of-words hashing embedder: each whitespace token lands
    /// in a bucket, then the vector is L2-normalized. Same words -> similar
    /// vectors, which is enough to exercise the vector search path offline.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut v = vec![0.0f32; self.dims];
            for token in text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|t| !t.is_empty())
            {
                let mut h: u64 = 1469598103934665603;
                for b in token.to_ascii_lowercase().bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(1099511628211);
                }
                v[(h as usize) % self.dims] += 1.0;
            }
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut v {
                    *x /= norm;
                }
            }
            out.push(v);
        }
        Ok(out)
    }
}
