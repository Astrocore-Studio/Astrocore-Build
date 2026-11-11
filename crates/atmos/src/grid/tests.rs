use super::*;

/// Комната w×h из клеток воздуха; стены по контуру наружу.
fn room(w: i32, h: i32) -> Layout {
    let mut l = Layout::default();
    for x in 0..w {
        for z in 0..h {
            l.cells.insert((x, z), CellKind::Air);
        }
    }
    for x in 0..w {
        l.blocked.insert(edge((x, 0), (x, -1)));
        l.blocked.insert(edge((x, h - 1), (x, h)));
    }
    for z in 0..h {
        l.blocked.insert(edge((0, z), (-1, z)));
        l.blocked.insert(edge((w - 1, z), (w, z)));
    }
    l
}

#[test]
fn a_sealed_room_keeps_its_air_and_goes_quiet() {
    let mut g = Grid::new();
    g.set_layout(room(4, 4));
    let before = g.total_moles();
    g.run_cycles(40);
    assert!((g.total_moles() - before).abs() < 1e-2);
    assert!((g.pressure((1, 1)) - ONE_ATMOSPHERE).abs() < 0.01);
    assert_eq!(g.active_tiles(), 0, "ровный воздух — активных клеток нет");
}

#[test]
fn gas_spreads_evenly_and_the_total_is_conserved() {
    let mut g = Grid::new();
    g.set_layout(room(6, 1));
    // Накачиваем крайнюю клетку лишним кислородом.
    let mut extra = GasMixture::standard_air();
    extra.multiply(5.0);
    g.merge_into((0, 0), &extra);
    let before = g.total_moles();
    let start_gap = g.pressure((0, 0)) - g.pressure((5, 0));
    assert!(start_gap > 100.0);
    g.run_cycles(60);
    let gap = (g.pressure((0, 0)) - g.pressure((5, 0))).abs();
    assert!(gap < start_gap * 0.2, "давление выравнивается: {gap}");
    assert!(
        (g.total_moles() - before).abs() < before * 1e-4,
        "газ не пропадает"
    );
    g.run_cycles(400);
    let (lo, hi) = (0..6).fold((f32::MAX, 0.0f32), |(lo, hi), x| {
        let p = g.pressure((x, 0));
        (lo.min(p), hi.max(p))
    });
    assert!(
        hi - lo < 1.0,
        "в итоге давление почти одинаково: {lo}..{hi}"
    );
}

#[test]
fn a_breach_to_space_vents_the_room_to_near_vacuum() {
    let mut l = room(4, 4);
    // Пробоина в северной стене: ребро открыто в космос.
    l.blocked.remove(&edge((1, 0), (1, -1)));
    let mut g = Grid::new();
    g.set_layout(l);
    assert!(g.is_space((1, -1)));
    g.run_cycles(10);
    let early = g.pressure((1, 0));
    assert!(early < ONE_ATMOSPHERE, "воздух уходит: {early}");
    g.run_cycles(2000);
    assert!(
        g.pressure((3, 3)) < HAZARD_LOW_PRESSURE,
        "{}",
        g.pressure((3, 3))
    );
    assert!(g.total_moles() < 10.0);
}

#[test]
fn closing_a_door_stops_the_leak_and_opening_it_lets_air_through() {
    // Две комнаты 3×3 с общей стеной по x=3 и дверью на (3,1).
    let mut l = Layout::default();
    for x in 0..6 {
        for z in 0..3 {
            l.cells.insert((x, z), CellKind::Air);
        }
    }
    for x in 0..6 {
        l.blocked.insert(edge((x, 0), (x, -1)));
        l.blocked.insert(edge((x, 2), (x, 3)));
    }
    for z in 0..3 {
        l.blocked.insert(edge((0, z), (-1, z)));
        l.blocked.insert(edge((5, z), (6, z)));
        l.blocked.insert(edge((2, z), (3, z)));
    }
    let mut g = Grid::new();
    g.set_layout(l.clone());
    // Вторая комната пуста.
    for x in 3..6 {
        for z in 0..3 {
            let all = g.air((x, z)).total_moles();
            g.remove_volume((x, z), CELL_VOLUME * (all / all.max(1e-6)));
        }
    }
    let vacuum = g.pressure((4, 1));
    assert!(vacuum < 1.0);
    g.run_cycles(60);
    assert!(
        g.pressure((4, 1)) < 1.0,
        "дверь закрыта — воздух не проходит"
    );
    // Открываем дверь.
    l.blocked.remove(&edge((2, 1), (3, 1)));
    g.set_layout(l);
    g.run_cycles(400);
    let (a, b) = (g.pressure((1, 1)), g.pressure((4, 1)));
    assert!(
        b > 20.0 && (a - b).abs() < 15.0,
        "воздух перетёк: {a} и {b}"
    );
}

#[test]
fn temperature_equalizes_between_neighbours() {
    let mut g = Grid::new();
    g.set_layout(room(2, 1));
    let mut hot = GasMixture::standard_air();
    hot.set_temperature(400.0);
    g.merge_into((0, 0), &hot);
    let hot_temp = g.air((0, 0)).temperature();
    assert!(hot_temp > 340.0);
    g.run_cycles(200);
    let (a, b) = (g.air((0, 0)).temperature(), g.air((1, 0)).temperature());
    assert!((a - b).abs() < 1.0, "{a} {b}");
    assert!(a < hot_temp && b > T20C);
}

#[test]
fn space_stays_a_vacuum_and_the_cycle_has_nine_steps() {
    let mut g = Grid::new();
    let mut l = room(1, 1);
    l.blocked.remove(&edge((0, 0), (0, -1)));
    g.set_layout(l);
    g.run_cycles(300);
    assert_eq!(g.air((0, -1)).total_moles(), 0.0);
    assert_eq!(g.air((0, -1)).temperature(), TCMB);
    let mut h = Grid::new();
    h.set_layout(room(1, 1));
    h.advance(ATMOS_TIME * 9.0 + 0.001);
    assert_eq!(h.cycles(), 1, "цикл — девять шагов по 1/15 с");
}

#[test]
fn a_breach_depressurizes_the_room_in_seconds_with_monstermos() {
    let mut l = room(5, 5);
    l.blocked.remove(&edge((2, 0), (2, -1)));
    let mut g = Grid::new();
    g.set_layout(l);
    let mut seconds = None;
    for c in 1..=400 {
        g.run_cycles(1);
        if g.pressure((2, 4)) < HAZARD_LOW_PRESSURE {
            seconds = Some(c as f32 * 9.0 / 15.0);
            break;
        }
    }
    let s = seconds.expect("комната опустела");
    assert!(s < 15.0, "взрывная разгерметизация быстрая: {s} с");
}

#[test]
fn equalization_moves_gas_between_distant_tiles_in_one_cycle_and_conserves_it() {
    let mut g = Grid::new();
    g.set_layout(room(8, 1));
    let mut extra = GasMixture::standard_air();
    extra.multiply(9.0);
    g.merge_into((0, 0), &extra);
    let before = g.total_moles();
    g.run_cycles(2);
    let spread = (0..8).map(|x| g.pressure((x, 0))).fold(0.0f32, f32::max)
        - (0..8).map(|x| g.pressure((x, 0))).fold(f32::MAX, f32::min);
    assert!(spread < 60.0, "после двух циклов почти ровно: {spread}");
    assert!(
        (g.total_moles() - before).abs() < before * 1e-3,
        "газ сохраняется"
    );
}

#[test]
fn a_stair_link_lets_air_flow_between_floors() {
    // Нижний и верхний этаж — по комнате 2×1; лестница соединяет (1,0) нижнего и (0,0) верхнего.
    let mut l = Layout::default();
    for floor in 0..2 {
        for x in 0..2 {
            l.cells.insert(floor_cell(floor, x, 0), CellKind::Air);
            l.blocked
                .insert(edge(floor_cell(floor, x, 0), floor_cell(floor, x, -1)));
            l.blocked
                .insert(edge(floor_cell(floor, x, 0), floor_cell(floor, x, 1)));
        }
        l.blocked
            .insert(edge(floor_cell(floor, 0, 0), floor_cell(floor, -1, 0)));
        l.blocked
            .insert(edge(floor_cell(floor, 1, 0), floor_cell(floor, 2, 0)));
    }
    let mut g = Grid::new();
    g.set_layout(l.clone());
    // Верхний этаж без воздуха.
    for x in 0..2 {
        g.remove_volume(floor_cell(1, x, 0), CELL_VOLUME);
    }
    g.run_cycles(60);
    assert!(
        g.pressure(floor_cell(1, 1, 0)) < 1.0,
        "без лестницы воздух не поднимается"
    );
    l.links.insert((floor_cell(0, 1, 0), floor_cell(1, 0, 0)));
    g.set_layout(l);
    g.run_cycles(300);
    let (low, high) = (
        g.pressure(floor_cell(0, 0, 0)),
        g.pressure(floor_cell(1, 1, 0)),
    );
    assert!(
        high > 20.0 && (low - high).abs() < 20.0,
        "воздух поднялся: {low} и {high}"
    );
}
