//! The rule of the thumbnail cache's limit. The cache itself (`rf-app/src/image_cache.rs`) is
//! memory only: decoded pictures of thumbnails and of per-file icons, kept least recently used
//! first. rustyfiles keeps no thumbnail files of its own, and Windows' own thumbnail cache (the
//! `thumbcache_*.db` files of the shell) is never read, written or deleted from here.
//!
//! What is here is pure: how the user's setting becomes a byte budget, what a tracked picture is
//! charged, and which pictures go when the budget is exceeded.

/// The budget when the user has not chosen one (what the cache always used).
pub const DEFAULT_MB: u32 = 64;
/// Smallest and largest limit the setting can hold; a value outside is cut to these.
pub const MIN_MB: u32 = 16;
pub const MAX_MB: u32 = 2048;
/// What the setting's menu offers.
pub const CHOICES_MB: [u32; 7] = [16, 32, 64, 128, 256, 512, 1024];

/// Minimum charge per tracked picture, so slots that are still pending or have failed are bounded
/// too (a folder of 100 000 files must not track 100 000 free slots).
pub const NOMINAL: usize = 16 * 1024;

/// What a picture of `bytes` pixel bytes is charged against the budget.
pub fn charge(bytes: usize) -> usize {
    bytes.max(NOMINAL)
}

/// The limit in megabytes the cache works with: the setting, cut to the range, or the default.
pub fn limit_mb(setting: Option<u32>) -> u32 {
    setting.map_or(DEFAULT_MB, |mb| mb.clamp(MIN_MB, MAX_MB))
}

/// The budget in bytes for a setting.
pub fn budget_bytes(setting: Option<u32>) -> usize {
    limit_mb(setting) as usize * 1024 * 1024
}

/// What to store in the settings file for a chosen limit: nothing for the default, so a user who
/// never touches the limit has no new key in the file.
pub fn setting_for(mb: u32) -> Option<u32> {
    (limit_mb(Some(mb)) != DEFAULT_MB).then(|| limit_mb(Some(mb)))
}

/// Which tracked pictures to drop. `items` are `(key, last use, pixel bytes)`; `total` is what
/// they are charged altogether. Within the budget nothing goes; over it, the least recently used
/// go until the total is down to 90 % of the budget (a batch, so that the cache does not sort
/// again on every frame it is a little over).
pub fn eviction_victims(items: &[(u64, u64, usize)], total: usize, budget: usize) -> Vec<u64> {
    if total <= budget {
        return Vec::new();
    }
    let target = budget / 10 * 9;
    let mut order: Vec<(u64, u64, usize)> = items.to_vec();
    order.sort_unstable_by_key(|&(_, tick, _)| tick);
    let mut left = total;
    let mut out = Vec::new();
    for (key, _, bytes) in order {
        if left <= target {
            break;
        }
        left -= charge(bytes).min(left);
        out.push(key);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_is_cut_to_the_range_and_nothing_means_the_old_budget() {
        assert_eq!(limit_mb(None), 64);
        assert_eq!(budget_bytes(None), 64 * 1024 * 1024);
        assert_eq!(limit_mb(Some(0)), MIN_MB);
        assert_eq!(limit_mb(Some(1)), MIN_MB);
        assert_eq!(limit_mb(Some(256)), 256);
        assert_eq!(limit_mb(Some(u32::MAX)), MAX_MB);
        assert_eq!(budget_bytes(Some(128)), 128 * 1024 * 1024);
        for c in CHOICES_MB {
            assert_eq!(limit_mb(Some(c)), c, "a choice the menu offers is kept as it is");
        }
    }

    #[test]
    fn the_default_limit_is_not_written_to_the_settings() {
        assert_eq!(setting_for(64), None);
        assert_eq!(setting_for(128), Some(128));
        // a value outside the range is stored as the range's edge, never as itself
        assert_eq!(setting_for(1), Some(MIN_MB));
        assert_eq!(setting_for(1_000_000), Some(MAX_MB));
    }

    #[test]
    fn a_tracked_picture_is_charged_at_least_the_nominal_amount() {
        assert_eq!(charge(0), NOMINAL);
        assert_eq!(charge(NOMINAL - 1), NOMINAL);
        assert_eq!(charge(300_000), 300_000);
    }

    #[test]
    fn within_the_budget_nothing_goes() {
        let items = [(1, 1, 100), (2, 2, 100)];
        assert!(eviction_victims(&items, 1000, 1000).is_empty());
        assert!(eviction_victims(&items, 10, 1000).is_empty());
        assert!(eviction_victims(&[], 5000, 1000).is_empty());
    }

    #[test]
    fn over_the_budget_the_least_recently_used_go_down_to_ninety_percent() {
        // ten pictures of 100 000 bytes, used in the order of their keys; budget 900 000
        let items: Vec<(u64, u64, usize)> = (0..10).map(|k| (k, k + 1, 100_000)).collect();
        let total = 10 * 100_000;
        let gone = eviction_victims(&items, total, 900_000);
        // target is 810 000: two go (the two oldest), 800 000 is left
        assert_eq!(gone, vec![0, 1]);
        // given in another order the result is the same: it is by last use, not by position
        let mut shuffled = items.clone();
        shuffled.reverse();
        assert_eq!(eviction_victims(&shuffled, total, 900_000), vec![0, 1]);
    }

    #[test]
    fn small_pictures_are_dropped_by_what_they_are_charged_not_by_their_size() {
        // 100 pending slots (0 bytes) are charged 16 KB each: 1 600 KB against a 1 000 KB budget
        let items: Vec<(u64, u64, usize)> = (0..100).map(|k| (k, k, 0)).collect();
        let total = 100 * NOMINAL;
        let gone = eviction_victims(&items, total, 1_000 * 1024);
        let left = total - gone.len() * NOMINAL;
        assert!(left <= 1_000 * 1024 / 10 * 9);
        assert!(left + NOMINAL > 1_000 * 1024 / 10 * 9, "no more than needed goes");
        assert_eq!(gone[0], 0, "the oldest goes first");
    }

    #[test]
    fn a_smaller_limit_than_the_cache_holds_frees_the_difference() {
        // 64 MB held, the user lowers the limit to 16 MB: down to 14.4 MB
        let items: Vec<(u64, u64, usize)> = (0..64).map(|k| (k, k, 1024 * 1024)).collect();
        let total = 64 * 1024 * 1024;
        let gone = eviction_victims(&items, total, budget_bytes(Some(16)));
        let left = total - gone.len() * 1024 * 1024;
        assert!(left <= budget_bytes(Some(16)) / 10 * 9);
        assert_eq!(gone.len(), 50);
    }
}
