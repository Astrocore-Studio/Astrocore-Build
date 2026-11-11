//! Швабры и вёдра (`SharedAbsorbentSystem`, `AbsorbentComponent`).
//!
//! Швабра (`Absorbent`) хранит раствор «absorbed» (120 ед.). Ударом по луже мокрая швабра меняет
//! чистую воду на грязь: из лужи забирается до `pickupAmount` единиц не-впитывающих реагентов, а
//! в лужу возвращается столько же воды из швабры. Без воды в швабре и на уже вымытой луже
//! ничего не происходит. По ёмкости (`Refillable`) швабра переливает: в пустую выливает грязь
//! (или воду), в непустую берёт воду и отдаёт грязь. Задержка между ударами — `UseDelay` 1 с.

use prototypes::{Data, Resolved};

use crate::puddles::is_absorbent;
use crate::reagents::Solution;

/// `UseDelay` швабры, с.
pub const MOP_DELAY_S: f32 = 1.0;
/// `PickupAmount` по умолчанию.
pub const DEFAULT_PICKUP: f32 = 120.0;

/// `Absorbent: (pickupAmount: …)` прототипа.
pub fn pickup_amount(r: &Resolved) -> Option<f32> {
    let c = r.component("Absorbent")?;
    Some(
        c.get("pickupAmount")
            .and_then(Data::as_f64)
            .map_or(DEFAULT_PICKUP, |v| v.max(0.0) as f32),
    )
}

pub fn is_refillable(r: &Resolved) -> bool {
    r.component("Refillable").is_some()
}

/// Результат действия швабры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MopResult {
    /// Что-то перелилось.
    Done,
    /// Нечего делать (нет воды, лужа уже чистая, некуда переливать).
    Nothing,
}

/// `TryPuddleInteract` при `UseAbsorberSolution`: обмен грязи лужи на воду швабры.
pub fn mop_puddle(absorber: &mut Solution, puddle: &mut Solution, pickup: f32) -> MopResult {
    let puddle_water = puddle.quantity_where(is_absorbent);
    if (puddle_water - puddle.volume()).abs() < 1e-4 {
        return MopResult::Nothing; // уже вымыто
    }
    let available = absorber.quantity_where(is_absorbent);
    if available <= 0.0 {
        return MopResult::Nothing; // нет воды
    }
    let transfer = available.min(pickup);
    let dirt = puddle.split_where(transfer, |r| !is_absorbent(r));
    let water = absorber.split_where(dirt.volume(), is_absorbent);
    puddle.merge(&water);
    absorber.merge(&dirt);
    MopResult::Done
}

/// `TryRefillableInteract`: швабра и ёмкость (ведро).
pub fn mop_container(absorber: &mut Solution, container: &mut Solution, pickup: f32) -> MopResult {
    if container.volume() <= 0.0 {
        // Из швабры в пустую ёмкость: сначала грязь, иначе вода.
        if absorber.volume() <= 0.0 {
            return MopResult::Nothing;
        }
        let amount = pickup.min(container.available());
        if amount <= 0.0 {
            return MopResult::Nothing;
        }
        let dirt = absorber.split_where(amount, |r| !is_absorbent(r));
        let moved = if dirt.volume() > 0.0 {
            dirt
        } else {
            absorber.split(amount)
        };
        container.merge(&moved);
        return MopResult::Done;
    }
    // Двусторонний обмен: грязь из швабры уходит в ёмкость, вода из ёмкости — в швабру.
    let mut dirt = absorber.split_where(pickup, |r| !is_absorbent(r));
    if dirt.volume() <= 0.0 && absorber.available() <= 0.0 {
        return MopResult::Nothing; // швабра полная и чистая
    }
    let water_pulled = pickup.min(absorber.available());
    let water = container.split_where(water_pulled, is_absorbent);
    if water.volume() <= 0.0 && dirt.volume() <= 0.0 {
        return MopResult::Nothing; // в ёмкости нет воды
    }
    let mut done = false;
    if water.volume() > 0.0 {
        absorber.merge(&water);
        done = true;
    }
    if dirt.volume() > 0.0 {
        if container.available() > 0.0 {
            let part = dirt.split(container.available());
            container.merge(&part);
            done = true;
        }
        absorber.merge(&dirt);
    }
    if done {
        MopResult::Done
    } else {
        MopResult::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sol(max: f32, parts: &[(&str, f32)]) -> Solution {
        let mut s = Solution::new(max);
        for (r, q) in parts {
            s.add(r, *q);
        }
        s
    }

    #[test]
    fn a_wet_mop_swaps_clean_water_for_the_dirt_in_a_puddle() {
        let mut mop = sol(120.0, &[("Water", 100.0)]);
        let mut puddle = sol(1e9, &[("Blood", 30.0)]);
        assert_eq!(mop_puddle(&mut mop, &mut puddle, 120.0), MopResult::Done);
        assert!((puddle.amount("Blood")).abs() < 1e-3, "кровь убрана");
        assert!(
            (puddle.amount("Water") - 30.0).abs() < 1e-3,
            "вместо неё вода"
        );
        assert!((mop.amount("Blood") - 30.0).abs() < 1e-3);
        assert!((mop.amount("Water") - 70.0).abs() < 1e-3);
        // Вторая попытка: лужа уже вымыта.
        assert_eq!(mop_puddle(&mut mop, &mut puddle, 120.0), MopResult::Nothing);
    }

    #[test]
    fn a_dry_mop_does_nothing_to_a_puddle() {
        let mut mop = Solution::new(120.0);
        let mut puddle = sol(1e9, &[("Blood", 30.0)]);
        assert_eq!(mop_puddle(&mut mop, &mut puddle, 120.0), MopResult::Nothing);
        assert!(puddle.amount("Blood") > 29.0);
    }

    #[test]
    fn a_mop_takes_water_from_a_bucket_and_gives_dirt_back() {
        let mut mop = Solution::new(120.0);
        let mut bucket = sol(360.0, &[("Water", 360.0)]);
        assert_eq!(mop_container(&mut mop, &mut bucket, 120.0), MopResult::Done);
        assert!((mop.amount("Water") - 120.0).abs() < 1e-3);
        assert!((bucket.amount("Water") - 240.0).abs() < 1e-3);
        // Испачкали и снова макаем: грязь уходит в ведро.
        mop.remove("Water", 20.0);
        mop.add("Blood", 20.0);
        assert_eq!(mop_container(&mut mop, &mut bucket, 120.0), MopResult::Done);
        assert!(mop.amount("Blood") < 1e-3, "грязь в ведре");
        assert!(bucket.amount("Blood") > 19.0);
    }

    #[test]
    fn an_empty_container_receives_what_the_mop_holds() {
        let mut mop = sol(120.0, &[("Water", 50.0), ("Blood", 10.0)]);
        let mut bucket = Solution::new(360.0);
        assert_eq!(mop_container(&mut mop, &mut bucket, 120.0), MopResult::Done);
        assert!(bucket.amount("Blood") > 9.0 && bucket.amount("Water") < 1e-3);
    }
}
