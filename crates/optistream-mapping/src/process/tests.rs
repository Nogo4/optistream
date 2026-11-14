use optistream_rage::texture::format as tf;

use crate::process::budget::Candidate;
use super::*;

#[test]
fn candidate_sizes_follow_halvings() {
    let c = Candidate { block: 0, format: tf::DXT1, width: 2048, height: 2048, levels: 12, loss: vec![0.0], tier: vec![0] };
    let full = c.size(0, true);
    let half = c.size(1, true);
    // DXT1 2048² full chain ≈ 2.67 Mio; one halving keeps about a quarter.
    assert!(full > 2_700_000 && full < 2_900_000, "{full}");
    assert!(half * 4 >= full - 64 && half * 4 <= full + 64, "{half}");
}

#[test]
fn kinds_from_names() {
    assert_eq!(Kind::of("a.ytd"), Some(Kind::Ytd));
    assert_eq!(Kind::of("b.ydr"), Some(Kind::Ydr));
    assert_eq!(Kind::of("c.ydd"), Some(Kind::Ydd));
    assert_eq!(Kind::of("d.yft"), None);
}
