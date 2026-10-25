/// Jenkins one-at-a-time hash as used by RAGE, on the lowercased input.
pub fn joaat(s: &str) -> u32 {
    let mut h: u32 = 0;
    for b in s.bytes() {
        h = h.wrapping_add(b.to_ascii_lowercase() as u32);
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}

#[cfg(test)]
mod tests {
    use super::joaat;

    #[test]
    fn known_hashes() {
        // Well-known GTA V hashes.
        assert_eq!(joaat("adder"), 0xB779_A091);
        assert_eq!(joaat("ADDER"), 0xB779_A091);
    }
}
