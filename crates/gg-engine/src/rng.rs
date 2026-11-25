//! Random numbers that draw alike on every machine.

use crate::Reflect;
use crate::ecs::splitmix64;

/// A seeded stream of random numbers: Sebastiano Vigna's SplitMix64.
///
/// A seed draws the same numbers on every platform. Keep it in a component, as
/// `deck: Rng` or a component of its own, so snapshots, recordings and restarts carry its state and
/// a seeded run replays alike. A scene seeds it: `rng: (state: 42)`.
///
/// Its numbers aren't secret: one gives away the rest of its stream.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Reflect)]
pub struct Rng {
    /// Where the stream is: the seed, until it draws.
    pub state: u64,
}

impl Rng {
    /// A stream from `seed`. Any seed is good, 0 too.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The stream's next number, any of the 2^64.
    pub fn next_u64(&mut self) -> u64 {
        splitmix64(&mut self.state)
    }

    /// A number from 0 up to `n`, `n` excluded, or 0 when `n` is 0.
    ///
    /// Lemire's multiply-shift: it's off from even by less than `n` in 2^64, which no game sees.
    pub fn below(&mut self, n: usize) -> usize {
        let wide = u128::from(self.next_u64()) * n as u128;
        // Less than `n`, so it fits.
        (wide >> 64) as usize
    }

    /// A number from 0 up to 1, 1 excluded, in steps of 2^-24: each `f32` there is as likely.
    pub fn unit(&mut self) -> f32 {
        // The top 24 bits, an `f32`'s precision: exact, so alike on every machine.
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// A number from `min` up to `max`, `max` excluded unless rounding lands on it.
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.unit()
    }

    /// Whether a draw falls under `p`: true with probability `p`, from 0 to 1.
    pub fn chance(&mut self, p: f32) -> bool {
        self.unit() < p
    }

    /// One of `items`, or `None` if there are none.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        items.get(self.below(items.len()))
    }

    /// Puts `items` in a random order, each order as likely: Fisher and Yates's shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for at in (1..items.len()).rev() {
            items.swap(at, self.below(at + 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    /// A seed's numbers are fixed: a change here changes every seeded game's runs.
    #[test]
    fn a_seed_draws_splitmix64s_numbers() {
        let mut rng = Rng::new(0);
        // splitmix64.c's first outputs from a state of 0.
        let first = [0xe220_a839_7b1d_cdaf, 0x6e78_9e6a_a1b9_65f4];
        assert_eq!([rng.next_u64(), rng.next_u64()], first);
    }

    #[test]
    fn draws_stay_in_their_ranges() {
        let mut rng = Rng::new(7);
        for _ in 0..10_000 {
            assert!(rng.below(6) < 6);
            let unit = rng.unit();
            assert!((0.0..1.0).contains(&unit), "{unit}");
            let x = rng.range(-2.0, 3.0);
            assert!((-2.0..=3.0).contains(&x), "{x}");
        }
        assert_eq!(rng.below(0), 0);
        assert_eq!(rng.pick::<u8>(&[]), None);
        assert!(!rng.chance(0.0));
        assert!(rng.chance(1.0));
    }

    #[test]
    fn below_reaches_each_number_about_evenly() {
        let mut rng = Rng::new(1);
        let mut counts = [0u32; 6];
        for _ in 0..60_000 {
            counts[rng.below(6)] += 1;
        }
        for count in counts {
            assert!((9_500..10_500).contains(&count), "{counts:?}");
        }
    }

    #[test]
    fn a_shuffle_keeps_its_items() {
        let mut rng = Rng::new(3);
        let mut items: Vec<u32> = (0..52).collect();
        rng.shuffle(&mut items);
        assert_ne!(items, (0..52).collect::<Vec<_>>());
        items.sort_unstable();
        assert_eq!(items, (0..52).collect::<Vec<_>>());
    }

    #[test]
    fn a_copy_draws_what_the_original_does() {
        let mut rng = Rng::new(9);
        rng.next_u64();
        let mut copy = rng;
        assert_eq!(copy.next_u64(), rng.next_u64());
    }
}
