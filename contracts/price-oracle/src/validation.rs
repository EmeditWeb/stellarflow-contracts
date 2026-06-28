/// Deviation threshold: 15% expressed in basis points (1500 bps).
const DEVIATION_THRESHOLD_BPS: i128 = 1500;

/// Compute the rolling baseline average from a slice of (timestamp, price) TWAP entries.
///
/// Returns `None` when the slice is empty.
fn baseline_average(entries: &soroban_sdk::Vec<(u64, i128)>) -> Option<i128> {
    let len = entries.len();
    if len == 0 {
        return None;
    }
    let mut sum: i128 = 0;
    for i in 0..len {
        let (_, price) = entries.get(i).unwrap();
        sum = sum.checked_add(price)?;
    }
    sum.checked_div(len as i128)
}

/// Returns `true` when `price` falls within ±15% of `baseline`.
///
/// Uses basis-point arithmetic to avoid floating-point:
///   deviation_bps = |price - baseline| * 10_000 / baseline
fn within_threshold(price: i128, baseline: i128) -> bool {
    if baseline <= 0 {
        return true; // no baseline yet — allow all
    }
    let delta = (price - baseline).unsigned_abs() as i128;
    // deviation_bps = delta * 10_000 / baseline
    match delta.checked_mul(10_000).and_then(|n| n.checked_div(baseline)) {
        Some(deviation_bps) => deviation_bps <= DEVIATION_THRESHOLD_BPS,
        None => false, // overflow means wildly out of range — reject
    }
}

/// Filter a set of validator feed submissions against the rolling baseline average.
///
/// Returns only the entries whose reported price falls within the ±15% variance
/// threshold. Feeds outside the threshold are silently dropped, preventing a
/// single compromised validator from skewing the consensus median.
///
/// If the TWAP buffer is empty (no baseline established yet), all entries are
/// kept so the oracle can bootstrap normally.
pub fn filter_feeds_by_deviation(
    twap_entries: &soroban_sdk::Vec<(u64, i128)>,
    feeds: soroban_sdk::Vec<crate::types::PriceBufferEntry>,
    env: &soroban_sdk::Env,
) -> soroban_sdk::Vec<crate::types::PriceBufferEntry> {
    let baseline = match baseline_average(twap_entries) {
        Some(b) => b,
        None => return feeds, // no history — pass all through
    };

    let mut accepted = soroban_sdk::Vec::new(env);
    for i in 0..feeds.len() {
        let entry = feeds.get(i).unwrap();
        if within_threshold(entry.price, baseline) {
            accepted.push_back(entry);
        }
    }
    accepted
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env};
    use crate::types::PriceBufferEntry;

    fn make_twap(env: &Env, prices: &[i128]) -> soroban_sdk::Vec<(u64, i128)> {
        let mut v = soroban_sdk::Vec::new(env);
        for (i, &p) in prices.iter().enumerate() {
            v.push_back((i as u64, p));
        }
        v
    }

    fn make_entry(env: &Env, price: i128) -> PriceBufferEntry {
        PriceBufferEntry {
            price,
            provider: Address::generate(env),
            timestamp: 0,
        }
    }

    #[test]
    fn passes_all_when_no_baseline() {
        let env = Env::default();
        let twap = soroban_sdk::Vec::new(&env);
        let mut feeds = soroban_sdk::Vec::new(&env);
        feeds.push_back(make_entry(&env, 999_999_999));
        feeds.push_back(make_entry(&env, 1));
        let result = filter_feeds_by_deviation(&twap, feeds.clone(), &env);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn keeps_feed_within_15_percent() {
        let env = Env::default();
        // baseline = 1_000_000_000
        let twap = make_twap(&env, &[1_000_000_000]);
        let mut feeds = soroban_sdk::Vec::new(&env);
        // exactly +15% => 1_150_000_000 — should be accepted
        feeds.push_back(make_entry(&env, 1_150_000_000));
        // exactly -15% => 850_000_000 — should be accepted
        feeds.push_back(make_entry(&env, 850_000_000));
        let result = filter_feeds_by_deviation(&twap, feeds, &env);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn drops_feed_outside_15_percent() {
        let env = Env::default();
        // baseline = 1_000_000_000
        let twap = make_twap(&env, &[1_000_000_000]);
        let mut feeds = soroban_sdk::Vec::new(&env);
        // +16% — should be dropped
        feeds.push_back(make_entry(&env, 1_160_000_000));
        // -16% — should be dropped
        feeds.push_back(make_entry(&env, 840_000_000));
        let result = filter_feeds_by_deviation(&twap, feeds, &env);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn mixed_feeds_only_valid_kept() {
        let env = Env::default();
        let twap = make_twap(&env, &[1_000_000_000, 1_010_000_000, 990_000_000]);
        // baseline avg = (1_000_000_000 + 1_010_000_000 + 990_000_000) / 3 = 1_000_000_000
        let mut feeds = soroban_sdk::Vec::new(&env);
        feeds.push_back(make_entry(&env, 1_050_000_000)); // +5% — keep
        feeds.push_back(make_entry(&env, 1_200_000_000)); // +20% — drop
        feeds.push_back(make_entry(&env, 950_000_000));   // -5% — keep
        let result = filter_feeds_by_deviation(&twap, feeds, &env);
        assert_eq!(result.len(), 2);
    }
}
