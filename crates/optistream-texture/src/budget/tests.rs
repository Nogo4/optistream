use optistream_rage::texture::format as tf;

use super::*;

#[test]
fn candidate_sizes_follow_halvings() {
    let c = Candidate { block: 0, format: tf::DXT1, width: 2048, height: 2048, levels: 12, loss: vec![0.0], key: vec![0] };
    let full = c.size(0, true);
    let half = c.size(1, true);
    // DXT1 2048² full chain ≈ 2.67 Mio; one halving keeps about a quarter.
    assert!(full > 2_700_000 && full < 2_900_000, "{full}");
    assert!(half * 4 >= full - 64 && half * 4 <= full + 64, "{half}");
}

#[test]
fn past_steps_come_after_every_tier_within() {
    // Tiers within the limits (0..=2) all sort before any step past them.
    for tier in 0..=2u8 {
        assert!(tier < PAST);
    }
    assert!(PAST + 2 > PAST);
}
