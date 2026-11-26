//! Случайные числа сервера (`IRobustRandom` Robust @ 1afd692): генератор xoshiro256** с
//! засевом SplitMix64. Ресурс мира; тесты задают зерно и получают повторяемый результат.

use bevy_ecs::resource::Resource;
use bevy_ecs::world::World;

/// Генератор. Ресурс `World`.
#[derive(Resource, Debug, Clone)]
pub struct GameRandom {
    state: [u64; 4],
}

impl GameRandom {
    pub fn seeded(seed: u64) -> Self {
        let mut x = seed;
        let mut next = || {
            x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = x;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        Self {
            state: [next(), next(), next(), next()],
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.state;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Число в `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    pub fn next_f32(&mut self) -> f32 {
        self.next_f64() as f32
    }

    /// Целое в `[min, max)` (`Next(min, max)`); при `max <= min` — `min`.
    pub fn range(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        let span = (i64::from(max) - i64::from(min)) as u64;
        (i64::from(min) + (self.next_u64() % span) as i64) as i32
    }

    /// Выпало ли событие с вероятностью `chance` (`Prob`).
    pub fn prob(&mut self, chance: f32) -> bool {
        self.next_f32() < chance
    }

    /// Номер варианта по весам (`SharedRandomExtensions.Pick`); `None` — все веса нулевые.
    pub fn pick_weighted(&mut self, weights: &[f32]) -> Option<usize> {
        let total: f32 = weights.iter().filter(|w| **w > 0.0).sum();
        if total <= 0.0 {
            return None;
        }
        let roll = self.next_f32() * total;
        let mut cumulative = 0.0;
        for (index, weight) in weights.iter().enumerate() {
            if *weight <= 0.0 {
                continue;
            }
            cumulative += weight;
            if roll < cumulative {
                return Some(index);
            }
        }
        weights.iter().rposition(|w| *w > 0.0)
    }
}

impl Default for GameRandom {
    /// Зерно от системного времени.
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x5EED, |time| time.as_nanos() as u64);
        Self::seeded(seed)
    }
}

/// Генератор мира; создаётся при первом обращении.
pub fn random(world: &mut World) -> bevy_ecs::world::Mut<'_, GameRandom> {
    world.get_resource_or_insert_with(GameRandom::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_gives_same_numbers_within_bounds() {
        let mut a = GameRandom::seeded(7);
        let mut b = GameRandom::seeded(7);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
            let r = a.range(-3, 4);
            assert!((-3..4).contains(&r));
            b.range(-3, 4);
            let f = a.next_f32();
            assert!((0.0..1.0).contains(&f));
            b.next_f32();
        }
        assert_eq!(a.range(5, 5), 5);
        assert_eq!(a.pick_weighted(&[0.0, 0.0]), None);
        assert_eq!(a.pick_weighted(&[0.0, 2.0, 0.0]), Some(1));
        let hits = (0..1000).filter(|_| a.prob(0.25)).count();
        assert!((180..320).contains(&hits), "{hits}");
    }
}
