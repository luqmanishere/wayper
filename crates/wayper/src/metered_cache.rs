use lru::LruCache;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use wgpu::naga::FastHashMap;

#[derive(Debug, Clone)]
pub struct CacheMetrics {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub size: usize,
    pub bytes: u64,
}

impl CacheMetrics {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total > 0 {
            (self.hits as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    }

    pub fn bytes_mb(&self) -> f64 {
        self.bytes as f64 / 1_048_576.0
    }
}

pub struct MeteredCache<K, V> {
    cache: LruCache<K, V>,
    sizes: FastHashMap<K, u64>,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    total_bytes: AtomicU64,
    total_inserted: AtomicUsize,
}

impl<K: Hash + Eq + Clone + std::fmt::Debug, V> MeteredCache<K, V> {
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            cache: LruCache::new(capacity),
            sizes: FastHashMap::default(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            total_inserted: AtomicUsize::new(0),
        }
    }

    pub fn contains(&self, key: &K) -> bool {
        self.cache.contains(key)
    }

    pub fn peek(&self, key: &K) -> Option<&V> {
        self.cache.peek(key)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.cache.contains(key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            self.cache.get(key)
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub fn get_or_insert(&mut self, key: K, size_bytes: u64, f: impl FnOnce() -> V) -> &V {
        if self.cache.contains(&key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return self.cache.get(&key).unwrap();
        }

        self.misses.fetch_add(1, Ordering::Relaxed);

        // Check if we're at capacity and will evict
        if self.cache.len() >= self.cache.cap().get() {
            if let Some((evicted_key, _)) = self.cache.peek_lru() {
                let evicted_key = evicted_key.clone();
                if let Some(evicted_size) = self.sizes.remove(&evicted_key) {
                    self.evictions.fetch_add(1, Ordering::Relaxed);
                    self.total_bytes
                        .fetch_sub(evicted_size, Ordering::Relaxed);
                    tracing::debug!(
                        key = ?evicted_key,
                        size_mb = evicted_size as f64 / 1_048_576.0,
                        "Cache eviction"
                    );
                }
            }
        }

        // Insert new entry
        self.sizes.insert(key.clone(), size_bytes);
        self.total_bytes
            .fetch_add(size_bytes, Ordering::Relaxed);
        self.total_inserted.fetch_add(1, Ordering::Relaxed);

        self.cache.get_or_insert(key, f)
    }

    pub fn metrics(&self) -> CacheMetrics {
        CacheMetrics {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            size: self.cache.len(),
            bytes: self.total_bytes.load(Ordering::Relaxed),
        }
    }

    pub fn total_inserted(&self) -> u64 {
        self.total_inserted.load(Ordering::Relaxed) as u64
    }
}
