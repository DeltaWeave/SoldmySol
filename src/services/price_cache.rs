use std::collections::HashMap;
use std::time::{Duration, Instant};

/// In-memory cache for token prices to reduce API calls
pub struct PriceCache {
    cache: HashMap<String, CachedPrice>,
    ttl: Duration,
}

struct CachedPrice {
    price: f64,
    timestamp: Instant,
}

impl PriceCache {
    /// Create a new price cache with specified TTL in seconds
    pub fn new(ttl_seconds: u64) -> Self {
        Self {
            cache: HashMap::new(),
            ttl: Duration::from_secs(ttl_seconds),
        }
    }

    /// Get cached price if not expired
    pub fn get(&self, token: &str) -> Option<f64> {
        self.cache.get(token).and_then(|cached| {
            if cached.timestamp.elapsed() < self.ttl {
                Some(cached.price)
            } else {
                None // Expired
            }
        })
    }

    /// Set/update cached price
    pub fn set(&mut self, token: &str, price: f64) {
        self.cache.insert(
            token.to_string(),
            CachedPrice {
                price,
                timestamp: Instant::now(),
            },
        );
    }

    /// Clean expired entries to prevent memory growth
    pub fn cleanup(&mut self) {
        self.cache.retain(|_, cached| cached.timestamp.elapsed() < self.ttl);
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> CacheStats {
        let total_entries = self.cache.len();
        let expired_entries = self.cache
            .values()
            .filter(|cached| cached.timestamp.elapsed() >= self.ttl)
            .count();

        CacheStats {
            total_entries,
            active_entries: total_entries - expired_entries,
            expired_entries,
        }
    }

    /// Clear all cached entries
    pub fn clear(&mut self) {
        self.cache.clear();
    }
}

pub struct CacheStats {
    pub total_entries: usize,
    pub active_entries: usize,
    pub expired_entries: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_cache_get_set() {
        let mut cache = PriceCache::new(60);

        cache.set("SOL", 150.0);
        assert_eq!(cache.get("SOL"), Some(150.0));

        cache.set("SOL", 155.0);
        assert_eq!(cache.get("SOL"), Some(155.0));
    }

    #[test]
    fn test_cache_expiration() {
        let mut cache = PriceCache::new(1); // 1 second TTL

        cache.set("SOL", 150.0);
        assert_eq!(cache.get("SOL"), Some(150.0));

        sleep(Duration::from_secs(2));
        assert_eq!(cache.get("SOL"), None); // Expired
    }

    #[test]
    fn test_cache_cleanup() {
        let mut cache = PriceCache::new(1);

        cache.set("SOL", 150.0);
        cache.set("ETH", 3000.0);

        sleep(Duration::from_secs(2));

        cache.cleanup();
        let stats = cache.get_stats();
        assert_eq!(stats.total_entries, 0); // All cleaned up
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = PriceCache::new(60);

        cache.set("SOL", 150.0);
        cache.set("ETH", 3000.0);
        cache.set("BTC", 50000.0);

        let stats = cache.get_stats();
        assert_eq!(stats.total_entries, 3);
        assert_eq!(stats.active_entries, 3);
        assert_eq!(stats.expired_entries, 0);
    }
}
