//! Shared helpers for the freminal fuzz targets.
//!
//! The fuzz targets themselves live in `fuzz_targets/`. This library only holds
//! logic that more than one target needs, so it is written once.

/// Split `data` into consecutive, non-empty chunks whose lengths are derived
/// from `seed`.
///
/// The split is a pure function of `(seed, scale, data.len())`, so a crashing
/// input reproduces exactly. The top six bits of `seed` pick the maximum chunk
/// length (`1..=64`, times `scale`); a small linear congruential generator
/// seeded with `seed` picks each individual length. Concatenating the returned
/// chunks always yields `data`.
///
/// Feeding the chunks to a stateful consumer one after another exercises its
/// chunk-boundary and resumption paths, which a single whole-input call never
/// reaches.
#[must_use]
pub fn split_chunks(seed: u8, scale: usize, data: &[u8]) -> Vec<&[u8]> {
    let max_len = (1 + usize::from(seed >> 2)).saturating_mul(scale.max(1));
    let mut state = u32::from(seed);
    let mut rest = data;
    let mut chunks = Vec::new();

    while !rest.is_empty() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        // The high bits of an LCG are the best distributed.
        let draw = usize::try_from(state >> 16).unwrap_or(0);
        let len = (1 + draw % max_len).min(rest.len());
        let (head, tail) = rest.split_at(len);
        chunks.push(head);
        rest = tail;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::split_chunks;

    #[test]
    fn chunks_concatenate_back_to_the_input() {
        let data: Vec<u8> = (0..=255).collect();
        for seed in [0u8, 1, 3, 64, 200, 255] {
            for scale in [1usize, 4] {
                let chunks = split_chunks(seed, scale, &data);
                assert!(chunks.iter().all(|c| !c.is_empty()));
                assert_eq!(chunks.concat(), data);
            }
        }
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        let expected: Vec<&[u8]> = Vec::new();
        assert_eq!(split_chunks(7, 1, &[]), expected);
    }

    #[test]
    fn split_is_deterministic() {
        let data = [0u8; 100];
        assert_eq!(split_chunks(42, 2, &data), split_chunks(42, 2, &data));
    }
}
