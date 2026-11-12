//! Дефибриллятор (`SharedDefibrillatorSystem`, `DefibrillatorComponent`; снимок SS14 `910154a7`).
//!
//! Включается «использованием в руке» (`ItemToggle`), пока включён, тянет `PowerCellDraw.DrawRate`
//! Дж/с из батареи; разряд требует `useCharge` (100 Дж, у медиум-батареи 720). Приложение к цели —
//! `DoAfter` на `doAfterDuration`, затем разряд: лечение `zapHeal` (в том числе мёртвому), удар током
//! `ZapDamage` и паралич на `WritheDuration`; мёртвый, чей урон опустился ниже порога смерти, оживает.
//! Откат между разрядами — `ZapDelay`.

use crate::health::DamageSpec;

#[derive(Debug, Clone)]
pub struct DefibSpec {
    pub heal: DamageSpec,
    pub zap_damage: f32,
    pub writhe: f32,
    pub zap_delay: f32,
    pub do_after: f32,
    /// Расход заряда на разряд; у предмета без батареи (`powered: false`) заряда не требуется.
    pub use_charge: f32,
    pub draw_rate: f32,
    pub charge: f32,
    pub powered: bool,
    pub toggled: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct DefibState {
    pub on: bool,
    pub charge: f32,
    pub cooldown: f32,
    /// Расход при включённом, Дж/с, и есть ли вообще батарея (копия из прототипа).
    pub draw: f32,
    pub powered: bool,
}

impl DefibSpec {
    pub fn parse(c: &prototypes::Data) -> Self {
        let f = |k: &str, d: f32| {
            c.get(k)
                .and_then(prototypes::Data::as_f64)
                .map_or(d, |v| v as f32)
        };
        let b = |k: &str| {
            c.get(k)
                .and_then(prototypes::Data::as_bool)
                .unwrap_or(false)
        };
        Self {
            heal: crate::module::damage_spec(c.get("zapHeal")),
            zap_damage: f("zapDamage", 5.0),
            writhe: f("writhe", 3.0),
            zap_delay: f("zapDelay", 5.0),
            do_after: f("doAfter", 3.0),
            use_charge: f("useCharge", 0.0),
            draw_rate: f("drawRate", 1.0),
            charge: f("charge", 0.0),
            powered: b("powered"),
            toggled: b("toggled"),
        }
    }

    pub fn fresh(&self) -> DefibState {
        DefibState {
            on: false,
            charge: self.charge,
            cooldown: 0.0,
            draw: if self.toggled { self.draw_rate } else { 0.0 },
            powered: self.powered,
        }
    }

    /// `HasActivatableCharge`: хватает ли на разряд.
    pub fn can_fire(&self, s: &DefibState) -> bool {
        !self.powered || s.charge >= self.use_charge
    }

    /// `HasDrawCharge` + `HasActivatableCharge` при включении.
    pub fn can_turn_on(&self, s: &DefibState) -> bool {
        !self.powered || (s.charge >= self.draw_rate && self.can_fire(s))
    }
}

impl DefibState {
    /// Шаг времени: откат убывает, включённый тянет заряд, а при нехватке выключается.
    pub fn tick(&mut self, dt: f32) {
        self.cooldown = (self.cooldown - dt).max(0.0);
        if self.on && self.powered {
            self.charge = (self.charge - self.draw * dt).max(0.0);
            if self.charge < self.draw.max(1e-3) {
                self.on = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> DefibSpec {
        DefibSpec {
            heal: DamageSpec::new(),
            zap_damage: 5.0,
            writhe: 3.0,
            zap_delay: 5.0,
            do_after: 3.0,
            use_charge: 100.0,
            draw_rate: 1.0,
            charge: 720.0,
            powered: true,
            toggled: true,
        }
    }

    #[test]
    fn an_empty_cell_cannot_fire_or_turn_on() {
        let s = spec();
        let mut st = s.fresh();
        assert!(s.can_turn_on(&st) && s.can_fire(&st));
        st.charge = 99.0;
        assert!(!s.can_fire(&st) && !s.can_turn_on(&st));
    }
}
