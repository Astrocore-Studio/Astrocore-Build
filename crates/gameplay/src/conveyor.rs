//! Конвейерные ленты (`SharedConveyorController`, `ConveyorSystem`, снимок SS14 `910154a7`).
//!
//! Лента 1×1 с `Conveyor: (speed: 2.0)` везёт предметы, чей центр лежит над ней: скорость задаёт
//! `Convey` — к осевой линии, если предмет сбоку, и вдоль ленты, если на оси. Лента работает при
//! состоянии не `Off` и при питании (`CanRun`); `Reverse` разворачивает направление. Состоянием
//! управляют выключатели (`SignalSwitch`: порт `On` → `Forward`, `Off` → `Off`).
//!
//! В отличие от SS14, где предметы — физические тела с разгоном 20 м/с² и трением, здесь предмет
//! сразу получает целевую скорость (разгон до 2 м/с занимает 0,1 с), а при выходе с ленты
//! останавливается. Игроки на ленте не движутся. Не перенесено: звук, утилизатор (`Recycler`).

/// `ConveyorState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Off,
    Forward,
    Reverse,
}

/// Направление ленты на плоскости (x, z): `rot` — четверти оборота против часовой от севера
/// (север — `z − 1`).
pub fn direction(rot: u8, state: State) -> [f32; 2] {
    let d = match rot % 4 {
        0 => [0.0, -1.0],
        1 => [-1.0, 0.0],
        2 => [0.0, 1.0],
        _ => [1.0, 0.0],
    };
    if state == State::Reverse {
        [-d[0], -d[1]]
    } else {
        d
    }
}

/// `Convey`: скорость предмета; `relative` — вектор от предмета к центру ленты.
pub fn convey(direction: [f32; 2], speed: f32, relative: [f32; 2]) -> [f32; 2] {
    let dd = direction[0] * direction[0] + direction[1] * direction[1];
    if speed == 0.0 || dd == 0.0 {
        return [0.0, 0.0];
    }
    // Проекция на направление и остаток: остаток ≈ 0 — предмет на осевой линии.
    let k = (relative[0] * direction[0] + relative[1] * direction[1]) / dd;
    let r = [
        relative[0] - direction[0] * k,
        relative[1] - direction[1] * k,
    ];
    let len = (r[0] * r[0] + r[1] * r[1]).sqrt();
    if len < 0.01 {
        return [direction[0] * speed, direction[1] * speed];
    }
    // Слегка вперёд (0,2), в основном к центру.
    let v = [r[0] + direction[0] * 0.2, r[1] + direction[1] * 0.2];
    let n = (v[0] * v[0] + v[1] * v[1]).sqrt();
    [v[0] / n * speed, v[1] / n * speed]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_turns_the_belt_counter_clockwise_from_north() {
        assert_eq!(direction(0, State::Forward), [0.0, -1.0]);
        assert_eq!(direction(1, State::Forward), [-1.0, 0.0]);
        assert_eq!(direction(2, State::Forward), [0.0, 1.0]);
        assert_eq!(direction(3, State::Forward), [1.0, 0.0]);
        assert_eq!(direction(0, State::Reverse), [0.0, 1.0]);
    }

    #[test]
    fn an_item_on_the_centre_line_runs_along_the_belt_at_belt_speed() {
        // Предмет южнее центра ленты, лента идёт на север: центр впереди, остаток 0.
        let v = convey([0.0, -1.0], 2.0, [0.0, -0.3]);
        assert_eq!(v, [0.0, -2.0]);
    }

    #[test]
    fn an_item_off_the_centre_line_is_pulled_towards_it() {
        // Предмет восточнее оси: центр на западе (−x).
        let v = convey([0.0, -1.0], 2.0, [-0.3, 0.0]);
        assert!(v[0] < 0.0 && v[1] < 0.0, "{v:?}");
        assert!(v[0].abs() > v[1].abs(), "в основном к центру: {v:?}");
        let speed = (v[0] * v[0] + v[1] * v[1]).sqrt();
        assert!((speed - 2.0).abs() < 1e-4);
    }

    #[test]
    fn a_stopped_belt_does_not_move_anything() {
        assert_eq!(convey([0.0, 0.0], 2.0, [0.1, 0.0]), [0.0, 0.0]);
        assert_eq!(convey([0.0, -1.0], 0.0, [0.1, 0.0]), [0.0, 0.0]);
    }
}
