//! E25: small layout cache with subtree-aware invalidation.

use std::collections::BTreeMap;

use super::ui_layout::UiRect;

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct LayoutCacheKey {
    pub scene_revision: u64,
    pub entity: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub invalidations: u64,
    pub entries: usize,
}

/// Caches resolved rectangles keyed by scene revision. Subtree invalidation
/// removes descendants as well as the changed node, matching the contract that
/// container changes affect child layout without invalidating unrelated trees.
#[derive(Debug, Default)]
pub struct UiLayoutCache {
    entries: BTreeMap<LayoutCacheKey, UiRect>,
    generations: BTreeMap<u64, u64>,
    stats: LayoutCacheStats,
}

impl UiLayoutCache {
    #[must_use]
    pub fn get(&mut self, scene_revision: u64, entity: u64) -> Option<UiRect> {
        let key = LayoutCacheKey {
            scene_revision,
            entity,
        };
        if self.entries.contains_key(&key) {
            self.stats.hits += 1;
            self.entries.get(&key).copied()
        } else {
            self.stats.misses += 1;
            None
        }
    }

    pub fn put(&mut self, scene_revision: u64, entity: u64, rect: UiRect) {
        self.entries.insert(
            LayoutCacheKey {
                scene_revision,
                entity,
            },
            rect,
        );
        self.stats.entries = self.entries.len();
    }

    /// Invalidates one node and its cached descendants.
    pub fn invalidate_subtree(&mut self, entity: u64, descendants: &[u64]) {
        self.bump_generation(entity);
        for id in descendants {
            self.bump_generation(*id);
        }
        self.stats.invalidations += 1 + descendants.len() as u64;
        self.stats.entries = self.entries.len();
    }

    pub fn invalidate_all(&mut self) {
        self.entries.clear();
        self.generations.clear();
        self.stats.invalidations += 1;
        self.stats.entries = 0;
    }

    /// A caller combines this generation with a scene revision when resolving
    /// whether an old entry is still usable.
    #[must_use]
    pub fn generation(&self, entity: u64) -> u64 {
        self.generations.get(&entity).copied().unwrap_or(0)
    }

    fn bump_generation(&mut self, entity: u64) {
        let generation = self.generation(entity).wrapping_add(1);
        self.generations.insert(entity, generation);
        self.entries.retain(|key, _| key.entity != entity);
    }

    #[must_use]
    pub const fn stats(&self) -> &LayoutCacheStats {
        &self.stats
    }

    pub fn reset_stats(&mut self) {
        self.stats.hits = 0;
        self.stats.misses = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_hits_and_reports_misses() {
        let mut cache = UiLayoutCache::default();
        assert!(cache.get(1, 10).is_none());
        cache.put(1, 10, UiRect::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(cache.get(1, 10), Some(UiRect::new(1.0, 2.0, 3.0, 4.0)));
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn invalidates_subtree_but_not_unrelated_entities() {
        let mut cache = UiLayoutCache::default();
        cache.put(1, 10, UiRect::new(0.0, 0.0, 1.0, 1.0));
        cache.put(1, 11, UiRect::new(0.0, 0.0, 2.0, 2.0));
        cache.invalidate_subtree(10, &[11]);
        assert!(cache.get(1, 10).is_none());
        assert!(cache.get(1, 11).is_none());
        assert_eq!(cache.generation(11), 1);
        cache.put(1, 12, UiRect::new(0.0, 0.0, 3.0, 3.0));
        assert_eq!(cache.generation(12), 0);
        assert_eq!(cache.stats().invalidations, 2);
    }

    #[test]
    fn invalidate_all_clears_entries_and_generations() {
        let mut cache = UiLayoutCache::default();
        cache.put(1, 1, UiRect::new(0.0, 0.0, 1.0, 1.0));
        cache.invalidate_all();
        assert!(cache.get(1, 1).is_none());
        assert_eq!(cache.generation(1), 0);
        assert_eq!(cache.stats().entries, 0);
    }
}
