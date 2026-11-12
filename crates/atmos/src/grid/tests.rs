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

/// Комната, где в клетке `(0,0)` плазма с кислородом.
fn plasma_room() -> Grid {
    let mut g = Grid::new();
    g.set_layout(room(5, 1));
    for x in 0..5 {
        g.with_air((x, 0), |a| {
            a.adjust_moles(Gas::Plasma, 40.0);
        });
    }
    g
}

#[test]
fn cold_plasma_and_oxygen_do_not_burn_by_themselves() {
    let mut g = plasma_room();
    g.run_cycles(60);
    assert!(g.fires().is_empty());
    assert!(g.air((2, 0)).moles(Gas::Plasma) > 39.0);
}

#[test]
fn a_spark_ignites_plasma_and_the_fire_spreads_and_burns_fuel() {
    let mut g = plasma_room();
    let plasma0: f32 = (0..5).map(|x| g.air((x, 0)).moles(Gas::Plasma)).sum();
    g.ignite((0, 0), 1500.0, 50.0);
    assert!(g.hotspot((0, 0)).is_some(), "искра зажигает смесь");
    g.run_cycles(400);
    let plasma: f32 = (0..5).map(|x| g.air((x, 0)).moles(Gas::Plasma)).sum();
    assert!(
        plasma < plasma0 * 0.97,
        "плазма горит: {plasma0} → {plasma}"
    );
    let co2: f32 = (0..5)
        .map(|x| g.air((x, 0)).moles(Gas::CarbonDioxide))
        .sum();
    assert!(co2 > 0.0, "при горении появляется углекислый газ");
    let hottest = (0..5)
        .map(|x| g.air((x, 0)).temperature())
        .fold(0.0, f32::max);
    assert!(hottest > 450.0, "горение греет воздух: {hottest}");
    assert!(
        g.hotspot((3, 0)).is_some() || g.air((3, 0)).temperature() > 400.0,
        "огонь перекинулся по комнате"
    );
}

#[test]
fn fire_goes_out_without_oxygen_or_fuel() {
    let mut g = Grid::new();
    g.set_layout(room(1, 1));
    g.with_air((0, 0), |a| {
        a.adjust_moles(Gas::Plasma, 30.0);
        a.set_moles(Gas::Oxygen, 0.0);
    });
    g.ignite((0, 0), 1500.0, 50.0);
    assert!(g.hotspot((0, 0)).is_none(), "без кислорода зажечь нельзя");
    g.with_air((0, 0), |a| a.adjust_moles(Gas::Oxygen, 20.0));
    g.ignite((0, 0), 1500.0, 50.0);
    assert!(g.hotspot((0, 0)).is_some());
    g.with_air((0, 0), |a| a.set_moles(Gas::Plasma, 0.0));
    g.run_cycles(10);
    assert!(g.hotspot((0, 0)).is_none(), "без топлива огонь гаснет");
}

#[test]
fn tritium_burns_into_water_vapor() {
    let mut mix = GasMixture::standard_air();
    mix.adjust_moles(Gas::Tritium, 10.0);
    mix.set_temperature(600.0);
    let out = reactions::react(&mut mix, false);
    assert!(out.reacting);
    assert!(mix.moles(Gas::WaterVapor) > 0.0);
    assert!(mix.moles(Gas::Tritium) < 10.0);
    assert!(mix.temperature() > 600.0, "горение выделяет тепло");
}

#[test]
fn frezon_cools_the_mixture_and_makes_nitrous_oxide() {
    let mut mix = GasMixture::standard_air();
    mix.adjust_moles(Gas::Frezon, 20.0);
    mix.set_temperature(300.0);
    let out = reactions::react(&mut mix, false);
    assert!(out.reacting);
    assert!(mix.temperature() < 300.0);
    assert!(mix.moles(Gas::NitrousOxide) > 0.0);
}

#[test]
fn hot_nitrous_oxide_decomposes_and_ammonia_burns_in_oxygen() {
    let mut mix = GasMixture::standard_air();
    mix.adjust_moles(Gas::NitrousOxide, 10.0);
    mix.set_temperature(900.0);
    reactions::react(&mut mix, false);
    assert!((mix.moles(Gas::NitrousOxide) - 5.0).abs() < 1e-3);

    let mut mix = GasMixture::standard_air();
    mix.adjust_moles(Gas::Ammonia, 30.0);
    mix.set_temperature(400.0);
    let before = mix.moles(Gas::Ammonia);
    reactions::react(&mut mix, false);
    assert!(mix.moles(Gas::Ammonia) < before);
    assert!(mix.moles(Gas::WaterVapor) > 0.0);
}

#[test]
fn the_reactions_run_in_priority_order() {
    let ids: Vec<i32> = reactions::REACTIONS.iter().map(|r| r.priority).collect();
    assert!(ids.windows(2).all(|w| w[0] >= w[1]));
}

mod piping {
    use super::*;
    use crate::pipes::{
        Canister, Device, DeviceEvent, EAST, NodeSpec, SOUTH, Scrubber, Tank, VentPump, WEST,
    };

    fn pipe(key: u64, x: i32, z: i32, dirs: u8) -> NodeSpec {
        NodeSpec {
            key,
            cell: (x, z),
            dirs,
            volume: 200.0,
        }
    }

    /// Труба вдоль строки `z` от `x0` до `x1` (ключи `base..`).
    fn line(base: u64, x0: i32, x1: i32, z: i32) -> Vec<NodeSpec> {
        (x0..=x1)
            .map(|x| pipe(base + (x - x0) as u64, x, z, EAST | WEST))
            .collect()
    }

    fn total_pipe_moles(g: &Grid, keys: &[u64]) -> f32 {
        let mut seen = Vec::new();
        let mut sum = 0.0;
        for &k in keys {
            let a = g.pipes().air_of(k).unwrap();
            let id = a as *const GasMixture as usize;
            if !seen.contains(&id) {
                seen.push(id);
                sum += a.total_moles();
            }
        }
        sum
    }

    #[test]
    fn touching_pipes_form_one_network_and_a_gap_splits_it() {
        let mut g = Grid::new();
        g.set_layout(room(6, 1));
        let mut specs = line(1, 0, 2, 0);
        specs.extend(line(10, 4, 5, 0));
        g.set_pipe_nodes(&specs);
        g.run_cycles(1);
        assert_eq!(g.pipes().net_count(), 2);
        assert!(std::ptr::eq(
            g.pipes().air_of(1).unwrap(),
            g.pipes().air_of(3).unwrap()
        ));
        assert_eq!(g.pipes().air_of(1).unwrap().volume, 600.0);
        // Пропущенный узел замыкает цепь.
        specs.push(pipe(99, 3, 0, EAST | WEST));
        g.set_pipe_nodes(&specs);
        g.run_cycles(1);
        assert_eq!(g.pipes().net_count(), 1);
    }

    #[test]
    fn gas_is_kept_when_networks_merge_and_split() {
        let mut g = Grid::new();
        g.set_layout(room(4, 1));
        let specs = line(1, 0, 3, 0);
        g.set_pipe_nodes(&specs);
        g.run_cycles(1);
        let mut fill = GasMixture::standard_air();
        fill.volume = 800.0;
        fill.multiply(0.5);
        g.pipes_mut().air_of_mut(1).unwrap().merge(&fill);
        let before = total_pipe_moles(&g, &[1, 2, 3, 4]);
        // Разрезаем посередине: газ делится поровну.
        let mut cut = specs.clone();
        cut.remove(2);
        g.set_pipe_nodes(&cut);
        g.run_cycles(1);
        let left = g.pipes().air_of(1).unwrap().total_moles();
        let right = g.pipes().air_of(4).unwrap().total_moles();
        assert!((left + right - before * 0.75).abs() < 0.01, "ушёл газ узла");
        assert!((left - 2.0 * right).abs() < 0.01, "делится по объёму");
    }

    #[test]
    fn a_pressure_pump_fills_the_outlet_up_to_its_target() {
        let mut g = Grid::new();
        g.set_layout(room(6, 1));
        // Вход: трубы 0..1, насос в клетке 2 (вход запад, выход восток), выход: 3..4.
        let mut specs = line(1, 0, 1, 0);
        specs.push(pipe(20, 2, 0, WEST));
        specs.push(pipe(21, 2, 0, EAST));
        specs.extend(line(30, 3, 4, 0));
        g.set_pipe_nodes(&specs);
        g.run_cycles(1);
        g.pipes_mut().add_device(
            500,
            (2, 0),
            Device::PressurePump {
                inlet: 20,
                outlet: 21,
                target: 300.0,
                enabled: true,
            },
        );
        g.pipes_mut()
            .air_of_mut(1)
            .unwrap()
            .adjust_moles(Gas::Oxygen, 150.0);
        let inlet_start = g.pipes().air_of(1).unwrap().pressure();
        g.run_cycles(20);
        let out = g.pipes().air_of(30).unwrap().pressure();
        assert!((out - 300.0).abs() < 5.0, "давление на выходе: {out}");
        assert!(g.pipes().air_of(1).unwrap().pressure() < inlet_start);
        // Выключенный насос ничего не качает.
        if let Some(Device::PressurePump {
            enabled, target, ..
        }) = g.pipes_mut().device_mut(500)
        {
            *enabled = false;
            *target = 1000.0;
        }
        g.run_cycles(10);
        assert!(g.pipes().air_of(30).unwrap().pressure() < 310.0);
    }

    #[test]
    fn a_canister_on_a_port_feeds_the_pipes_and_its_valve_vents_into_the_room() {
        let mut g = Grid::new();
        g.set_layout(room(5, 1));
        let mut specs = vec![pipe(1, 0, 0, EAST)];
        specs.extend(line(2, 1, 3, 0));
        g.set_pipe_nodes(&specs);
        let mut air = GasMixture::new(1500.0);
        air.set_temperature(T20C);
        air.adjust_moles(Gas::Oxygen, 1000.0);
        let mut can = Canister::new(air);
        can.port = Some(1);
        g.pipes_mut()
            .add_device(700, (0, 0), Device::Canister(Box::new(can)));
        g.run_cycles(10);
        let in_pipes = g.pipes().air_of(3).unwrap().total_moles();
        assert!(in_pipes > 50.0, "баллон отдал газ в трубы: {in_pipes}");
        // Клапан: открываем выпуск в помещение при давлении 300 кПа.
        if let Some(Device::Canister(c)) = g.pipes_mut().device_mut(700) {
            c.port = None;
            c.release_open = true;
            c.release_pressure = 300.0;
        }
        let before = g.pressure((0, 0));
        g.run_cycles(60);
        assert!(g.pressure((0, 0)) > before + 20.0, "{}", g.pressure((0, 0)));
    }

    #[test]
    fn a_vent_pressurizes_a_vacuum_and_a_scrubber_cleans_the_room() {
        let mut g = Grid::new();
        g.set_layout(room(3, 1));
        // Опустошаем центральную клетку и накачиваем через вентиляцию.
        for x in 0..3 {
            g.remove_volume((x, 0), CELL_VOLUME);
        }
        g.set_pipe_nodes(&[pipe(1, 1, 0, SOUTH)]);
        g.run_cycles(1);
        let mut gas = GasMixture::standard_air();
        gas.volume = 200.0;
        gas.multiply(40.0);
        g.pipes_mut().air_of_mut(1).unwrap().merge(&gas);
        g.pipes_mut()
            .add_device(5, (1, 0), Device::VentPump(VentPump::new(1)));
        g.run_cycles(120);
        assert!(g.pressure((1, 0)) > 40.0, "{}", g.pressure((1, 0)));

        // Скруббер вытягивает плазму.
        let mut g = Grid::new();
        g.set_layout(room(3, 1));
        for x in 0..3 {
            g.with_air((x, 0), |a| a.adjust_moles(Gas::Plasma, 10.0));
        }
        g.set_pipe_nodes(&[pipe(1, 1, 0, SOUTH)]);
        g.pipes_mut()
            .add_device(6, (1, 0), Device::Scrubber(Scrubber::new(1)));
        g.run_cycles(200);
        let plasma: f32 = (0..3).map(|x| g.air((x, 0)).moles(Gas::Plasma)).sum();
        assert!(plasma < 15.0, "плазма ушла в трубу: {plasma}");
        assert!(g.pipes().air_of(1).unwrap().moles(Gas::Plasma) > 5.0);
        assert!(g.air((1, 0)).moles(Gas::Oxygen) > 10.0, "воздух не откачан");
    }

    #[test]
    fn an_open_valve_joins_two_pipes_and_a_closed_one_does_not() {
        let mut g = Grid::new();
        g.set_layout(room(5, 1));
        let mut specs = line(1, 0, 1, 0);
        specs.push(pipe(20, 2, 0, WEST));
        specs.push(pipe(21, 2, 0, EAST));
        specs.extend(line(30, 3, 4, 0));
        g.set_pipe_nodes(&specs);
        g.pipes_mut().add_device(
            9,
            (2, 0),
            Device::Valve {
                a: 20,
                b: 21,
                open: false,
            },
        );
        g.run_cycles(1);
        assert_eq!(g.pipes().net_count(), 2);
        if let Some(Device::Valve { open, .. }) = g.pipes_mut().device_mut(9) {
            *open = true;
        }
        g.run_cycles(1);
        assert_eq!(g.pipes().net_count(), 1);
    }

    #[test]
    fn a_passive_vent_equalizes_pipe_and_room() {
        let mut g = Grid::new();
        g.set_layout(room(1, 1));
        g.set_pipe_nodes(&[pipe(1, 0, 0, SOUTH)]);
        g.run_cycles(1);
        let mut gas = GasMixture::standard_air();
        gas.volume = 200.0;
        gas.multiply(0.2);
        g.pipes_mut().air_of_mut(1).unwrap().merge(&gas);
        g.pipes_mut()
            .add_device(2, (0, 0), Device::PassiveVent { node: 1 });
        g.run_cycles(5);
        let (pipe_p, room_p) = (g.pipes().air_of(1).unwrap().pressure(), g.pressure((0, 0)));
        assert!((pipe_p - room_p).abs() < 1.0, "{pipe_p} против {room_p}");
    }

    #[test]
    fn a_tank_releases_gas_slowly_and_ruptures_over_pressure() {
        let mut mix = GasMixture::new(5.0);
        mix.set_temperature(T20C);
        mix.adjust_moles(Gas::Oxygen, 0.5);
        let mut tank = Tank::new(mix);
        let mut env = GasMixture::standard_air();
        // Закрытый баллончик ничего не теряет.
        tank.update(0.6, Some(&mut env));
        assert!((tank.air.total_moles() - 0.5).abs() < 1e-4);
        tank.release_open = true;
        let start = tank.air.total_moles();
        for _ in 0..20 {
            tank.update(0.6, Some(&mut env));
        }
        assert!(tank.air.total_moles() < start, "газ вышел");
        // Перегрузка: давление выше 20 атм ломает за `integrity` секунд, выше (integrity+1) раз — сразу.
        let mut mix = GasMixture::new(5.0);
        mix.adjust_moles(Gas::Oxygen, 5.0);
        mix.set_temperature(2000.0);
        let mut tank = Tank::new(mix);
        let status = tank.update(0.6, Some(&mut env));
        assert_eq!(status, crate::pipes::PressureStatus::Ruptured);
        assert_eq!(tank.air.total_moles(), 0.0);
    }

    #[test]
    fn a_canister_over_its_limit_opens_the_safety_valve_and_a_weak_one_bursts() {
        // Выше предохранительного давления клапан сам стравливает газ в помещение.
        let mut g = Grid::new();
        g.set_layout(room(2, 1));
        let mut air = GasMixture::new(1500.0);
        air.set_temperature(T20C);
        air.adjust_moles(Gas::Oxygen, 9000.0); // ≈ 146 атм
        g.pipes_mut()
            .add_device(1, (0, 0), Device::Canister(Box::new(Canister::new(air))));
        let before = g.total_moles();
        g.run_cycles(40);
        let Some(Device::Canister(c)) = g.pipes().device(1) else {
            panic!("баллон цел");
        };
        assert!(c.safety_open, "предохранительный клапан открыт");
        assert!(
            c.air.total_moles() < 9000.0 - 20.0,
            "клапан стравливает газ"
        );
        assert!(g.total_moles() > before, "газ вышел в помещение");

        // Изношенный баллон (целостность 0) лопается при давлении выше перегрузки.
        let mut g = Grid::new();
        g.set_layout(room(2, 1));
        let mut air = GasMixture::new(1500.0);
        air.set_temperature(T20C);
        air.adjust_moles(Gas::Oxygen, 20_000.0);
        let mut can = Canister::new(air);
        can.limits.integrity = 0.0;
        g.pipes_mut()
            .add_device(1, (0, 0), Device::Canister(Box::new(can)));
        let before = g.total_moles();
        g.run_cycles(2);
        let events = g.take_device_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, DeviceEvent::Ruptured { key: 1, .. })),
            "баллон лопнул: {events:?}"
        );
        assert!(g.total_moles() > before + 15_000.0, "газ ушёл в помещение");
        assert!(g.pipes().device(1).is_none());
    }
}

/// Замер на карте размером со станцию (≈4000 клеток): `cargo test -p atmos --release -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_station_sized_grid() {
    use std::time::Instant;
    let (w, h) = (64, 64);
    let mut l = room(w, h);
    // Коридорные перегородки: каждые 8 клеток стена с проёмом, как отсеки станции.
    for x in (8..w).step_by(8) {
        for z in 0..h {
            if z % 8 != 3 {
                l.blocked.insert(edge((x - 1, z), (x, z)));
            }
        }
    }
    let mut g = Grid::new();
    let t = Instant::now();
    g.set_layout(l.clone());
    println!("set_layout {} клеток: {:?}", w * h, t.elapsed());
    let t = Instant::now();
    g.run_cycles(50);
    println!(
        "покой: {:.3} мс на цикл",
        t.elapsed().as_secs_f64() * 1000.0 / 50.0
    );
    // Пробоина в космос: ребро наружу в углу.
    l.blocked.remove(&edge((0, 0), (0, -1)));
    g.set_layout(l);
    let t = Instant::now();
    let mut worst = 0.0f64;
    for _ in 0..600 {
        let s = Instant::now();
        g.run_cycles(1);
        worst = worst.max(s.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "разгерметизация: {:.3} мс на цикл в среднем, худший {:.2} мс, активных клеток {}",
        t.elapsed().as_secs_f64() * 1000.0 / 600.0,
        worst,
        g.active_tiles()
    );
    // Пожар в плазменной комнате.
    for x in 0..8 {
        for z in 0..8 {
            g.with_air((x, z), |a| a.adjust_moles(Gas::Plasma, 20.0));
        }
    }
    g.ignite((4, 4), 1500.0, 50.0);
    let t = Instant::now();
    g.run_cycles(300);
    println!(
        "пожар: {:.3} мс на цикл, горит клеток {}",
        t.elapsed().as_secs_f64() * 1000.0 / 300.0,
        g.fires().len()
    );
}

#[test]
fn a_wide_net_scrubber_also_cleans_the_neighbouring_cells() {
    use crate::pipes::{Device, NodeSpec, SOUTH, Scrubber};
    let build = |wide: bool| {
        let mut g = Grid::new();
        g.set_layout(room(3, 1));
        g.with_air((2, 0), |a| a.adjust_moles(Gas::Plasma, 20.0));
        g.set_pipe_nodes(&[NodeSpec {
            key: 1,
            cell: (1, 0),
            dirs: SOUTH,
            volume: 200.0,
        }]);
        let mut sc = Scrubber::new(1);
        sc.wide_net = wide;
        g.pipes_mut().add_device(6, (1, 0), Device::Scrubber(sc));
        g
    };
    let (mut narrow, mut wide) = (build(false), build(true));
    narrow.run_cycles(3);
    wide.run_cycles(3);
    let in_pipe = |g: &Grid| g.pipes().air_of(1).unwrap().moles(Gas::Plasma);
    assert!(
        in_pipe(&wide) > in_pipe(&narrow),
        "{} против {}",
        in_pipe(&wide),
        in_pipe(&narrow)
    );
}
