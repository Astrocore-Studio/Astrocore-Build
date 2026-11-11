//! Решатель `BatteryRampPegSolver` и `PowerSolverShared.UpdateRampPositions` из SS14.

/// Источник (`PowerState.Supply`).
#[derive(Debug, Clone)]
pub struct Supply {
    pub enabled: bool,
    pub max_supply: f32,
    pub ramp_rate: f32,
    pub ramp_tolerance: f32,
    pub ramp_position: f32,
    pub current_supply: f32,
    pub ramp_target: f32,
    pub available_supply: f32,
}

impl Supply {
    pub fn new(max_supply: f32) -> Self {
        Self {
            enabled: true,
            max_supply,
            ramp_rate: 5000.0,
            ramp_tolerance: 5000.0,
            ramp_position: 0.0,
            current_supply: 0.0,
            ramp_target: 0.0,
            available_supply: 0.0,
        }
    }
}

/// Потребитель (`PowerState.Load`).
#[derive(Debug, Clone)]
pub struct Load {
    pub enabled: bool,
    pub desired_power: f32,
    pub receiving_power: f32,
}

impl Load {
    pub fn new(desired_power: f32) -> Self {
        Self {
            enabled: true,
            desired_power,
            receiving_power: 0.0,
        }
    }
}

/// Аккумулятор (`PowerState.Battery`): заряжается из `charging`, разряжается в `discharging`.
#[derive(Debug, Clone)]
pub struct Battery {
    pub enabled: bool,
    pub can_discharge: bool,
    pub can_charge: bool,
    pub capacity: f32,
    pub max_charge_rate: f32,
    pub max_supply: f32,
    pub supply_ramp_tolerance: f32,
    pub supply_ramp_rate: f32,
    pub efficiency: f32,
    pub supply_ramp_position: f32,
    pub current_supply: f32,
    pub current_storage: f32,
    pub current_receiving: f32,
    pub loading_network_demand: f32,
    supplying_marked: bool,
    loading_marked: bool,
    available_supply: f32,
    desired_power: f32,
    supply_ramp_target: f32,
    max_effective_supply: f32,
    /// Сеть, из которой заряжается, и сеть, в которую отдаёт.
    pub charging: Option<usize>,
    pub discharging: Option<usize>,
}

impl Battery {
    pub fn new(capacity: f32, storage: f32) -> Self {
        Self {
            enabled: true,
            can_discharge: true,
            can_charge: true,
            capacity,
            max_charge_rate: 0.0,
            max_supply: 0.0,
            supply_ramp_tolerance: 5000.0,
            supply_ramp_rate: 5000.0,
            efficiency: 1.0,
            supply_ramp_position: 0.0,
            current_supply: 0.0,
            current_storage: storage,
            current_receiving: 0.0,
            loading_network_demand: 0.0,
            supplying_marked: false,
            loading_marked: false,
            available_supply: 0.0,
            desired_power: 0.0,
            supply_ramp_target: 0.0,
            max_effective_supply: 0.0,
            charging: None,
            discharging: None,
        }
    }
}

/// Сеть: что в неё включено.
#[derive(Debug, Clone, Default)]
pub struct Network {
    pub supplies: Vec<usize>,
    pub loads: Vec<usize>,
    pub battery_loads: Vec<usize>,
    pub battery_supplies: Vec<usize>,
    pub last_combined_load: f32,
    pub last_combined_supply: f32,
    pub last_combined_max_supply: f32,
    height: i32,
}

#[derive(Debug, Default)]
pub struct PowerState {
    pub supplies: Vec<Supply>,
    pub loads: Vec<Load>,
    pub batteries: Vec<Battery>,
    pub networks: Vec<Network>,
    groups: Option<Vec<Vec<usize>>>,
}

impl PowerState {
    pub fn add_network(&mut self) -> usize {
        self.networks.push(Network::default());
        self.groups = None;
        self.networks.len() - 1
    }

    pub fn add_supply(&mut self, net: usize, s: Supply) -> usize {
        self.supplies.push(s);
        let id = self.supplies.len() - 1;
        self.networks[net].supplies.push(id);
        id
    }

    pub fn add_load(&mut self, net: usize, l: Load) -> usize {
        self.loads.push(l);
        let id = self.loads.len() - 1;
        self.networks[net].loads.push(id);
        id
    }

    /// Аккумулятор, заряжающийся из `charging` и отдающий в `discharging`.
    pub fn add_battery(&mut self, charging: usize, discharging: usize, mut b: Battery) -> usize {
        b.charging = Some(charging);
        b.discharging = Some(discharging);
        self.batteries.push(b);
        let id = self.batteries.len() - 1;
        self.networks[charging].battery_loads.push(id);
        self.networks[discharging].battery_supplies.push(id);
        self.groups = None;
        id
    }

    /// Шаг решателя на `dt` секунд (`BatteryRampPegSolver.Tick`).
    pub fn tick(&mut self, dt: f32) {
        for l in &mut self.loads {
            l.receiving_power = 0.0;
        }
        for s in &mut self.supplies {
            s.current_supply = 0.0;
            s.ramp_target = 0.0;
        }
        if self.groups.is_none() {
            self.groups = Some(self.group_by_depth());
        }
        let groups = self.groups.take().unwrap_or_default();
        for group in &groups {
            for &n in group {
                self.update_network(n, dt);
            }
        }
        self.groups = Some(groups);
        for b in &mut self.batteries {
            if !b.supplying_marked {
                b.current_supply = 0.0;
                b.supply_ramp_target = 0.0;
                b.loading_network_demand = 0.0;
            }
            if !b.loading_marked {
                b.current_receiving = 0.0;
            }
            b.supplying_marked = false;
            b.loading_marked = false;
        }
        self.update_ramp_positions(dt);
    }

    fn update_network(&mut self, n: usize, dt: f32) {
        let net = self.networks[n].clone();
        let mut demand = 0.0f32;
        for &l in &net.loads {
            let load = &self.loads[l];
            if load.enabled {
                demand += load.desired_power;
            }
        }
        for &b in &net.battery_loads {
            let battery = &mut self.batteries[b];
            if !battery.enabled || !battery.can_charge {
                continue;
            }
            let space = ((battery.capacity - battery.current_storage) * (1.0 / battery.efficiency))
                .max(0.0);
            let scaled_space = space / dt;
            let charge_rate = battery.max_charge_rate + battery.loading_network_demand / battery.efficiency;
            battery.desired_power = charge_rate.min(scaled_space);
            demand += battery.desired_power;
        }
        let (mut total_supply, mut total_max_supply) = (0.0f32, 0.0f32);
        for &s in &net.supplies {
            let supply = &mut self.supplies[s];
            if !supply.enabled {
                continue;
            }
            let ramp_max = supply.ramp_position + supply.ramp_tolerance;
            let effective = ramp_max.min(supply.max_supply);
            supply.available_supply = effective;
            total_supply += effective;
            total_max_supply += supply.max_supply;
        }
        let unmet = (demand - total_supply).max(0.0);
        let (mut total_battery_supply, mut total_max_battery_supply) = (0.0f32, 0.0f32);
        if unmet > 0.0 {
            for &b in &net.battery_supplies {
                let battery = &mut self.batteries[b];
                if !battery.enabled || !battery.can_discharge {
                    continue;
                }
                let scaled_space = battery.current_storage / dt;
                let supply_cap = battery
                    .max_supply
                    .min(battery.supply_ramp_position + battery.supply_ramp_tolerance);
                let passthrough = supply_cap + battery.current_receiving * battery.efficiency;
                battery.available_supply = scaled_space.min(passthrough);
                battery.loading_network_demand = unmet;
                battery.max_effective_supply = (battery.current_storage / dt)
                    .min(battery.max_supply + battery.current_receiving * battery.efficiency);
                total_battery_supply += battery.available_supply;
                total_max_battery_supply += battery.max_effective_supply;
            }
        }
        {
            let network = &mut self.networks[n];
            network.last_combined_load = demand;
            network.last_combined_supply = total_supply + total_battery_supply;
            network.last_combined_max_supply = total_max_supply + total_max_battery_supply;
        }
        let met = demand.min(total_supply + total_battery_supply);
        if met == 0.0 {
            return;
        }
        let supply_ratio = met / demand;
        for &l in &net.loads {
            let load = &mut self.loads[l];
            if !load.enabled || load.desired_power == 0.0 {
                continue;
            }
            load.receiving_power = load.desired_power * supply_ratio;
        }
        for &b in &net.battery_loads {
            let battery = &mut self.batteries[b];
            if !battery.enabled || battery.desired_power == 0.0 || !battery.can_charge {
                continue;
            }
            battery.loading_marked = true;
            battery.current_receiving = battery.desired_power * supply_ratio;
            battery.current_storage += dt * battery.current_receiving * battery.efficiency;
            battery.current_storage = battery.current_storage.min(battery.capacity);
        }
        let met_supply = demand.min(total_supply);
        if met_supply > 0.0 {
            let relative = met_supply / total_supply;
            let target_relative = demand.min(total_max_supply) / total_max_supply;
            for &s in &net.supplies {
                let supply = &mut self.supplies[s];
                if !supply.enabled {
                    continue;
                }
                supply.current_supply = supply.available_supply * relative;
                supply.ramp_target = supply.max_supply * target_relative;
            }
        }
        if unmet <= 0.0 || total_max_battery_supply <= 0.0 {
            return;
        }
        let relative_battery = unmet.min(total_battery_supply) / total_battery_supply;
        let relative_target = unmet.min(total_max_battery_supply) / total_max_battery_supply;
        for &b in &net.battery_supplies {
            let battery = &mut self.batteries[b];
            if !battery.enabled || !battery.can_discharge {
                continue;
            }
            battery.supplying_marked = true;
            battery.current_supply = battery.available_supply * relative_battery;
            battery.current_storage -= dt * battery.current_supply;
            battery.current_storage = battery.current_storage.max(0.0);
            battery.supply_ramp_target = battery.max_effective_supply * relative_target
                - battery.current_receiving * battery.efficiency;
        }
    }

    /// `PowerSolverShared.UpdateRampPositions`.
    fn update_ramp_positions(&mut self, dt: f32) {
        fn ramp(position: f32, target: f32, rate: f32, max: f32, dt: f32) -> f32 {
            let dev = target - position;
            if dev.abs() > 0.001 {
                let new = if dev > 0.0 {
                    target.min(position + rate * dt)
                } else {
                    target.max(position - rate * dt)
                };
                new.clamp(0.0, max)
            } else {
                target
            }
        }
        for s in &mut self.supplies {
            if !s.enabled {
                s.ramp_position = 0.0;
                continue;
            }
            s.ramp_position = ramp(s.ramp_position, s.ramp_target, s.ramp_rate, s.max_supply, dt);
        }
        for b in &mut self.batteries {
            if !b.enabled {
                b.supply_ramp_position = 0.0;
                continue;
            }
            b.supply_ramp_position = ramp(
                b.supply_ramp_position,
                b.supply_ramp_target,
                b.supply_ramp_rate,
                b.max_supply,
                dt,
            );
        }
    }

    /// `GroupByNetworkDepth`: высота сети = 1 + наибольшая высота сетей, в которые отдают её аккумуляторы.
    fn group_by_depth(&mut self) -> Vec<Vec<usize>> {
        for n in &mut self.networks {
            n.height = -1;
        }
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for n in 0..self.networks.len() {
            if self.networks[n].height == -1 {
                self.estimate_depth(n, &mut groups);
            }
        }
        groups
    }

    fn estimate_depth(&mut self, n: usize, groups: &mut Vec<Vec<usize>>) {
        self.networks[n].height = -2;
        let mut height = -1;
        for b in self.networks[n].battery_loads.clone() {
            let Some(sub) = self.batteries[b].discharging else {
                continue;
            };
            if sub == n {
                continue;
            }
            if self.networks[sub].height == -1 {
                self.estimate_depth(sub, groups);
            } else if self.networks[sub].height == -2 {
                continue;
            }
            height = height.max(self.networks[sub].height);
        }
        let h = 1 + height;
        self.networks[n].height = h;
        let h = h as usize;
        if h >= groups.len() {
            groups.push(vec![n]);
        } else {
            groups[h].push(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(state: &mut PowerState, seconds: f32) {
        let dt = 1.0 / 20.0;
        let mut t = 0.0;
        while t < seconds {
            state.tick(dt);
            t += dt;
        }
    }

    #[test]
    fn a_generator_feeds_a_load_after_it_ramps_up() {
        let mut p = PowerState::default();
        let n = p.add_network();
        let mut g = Supply::new(3000.0);
        g.ramp_rate = 500.0;
        g.ramp_tolerance = 500.0;
        let g = p.add_supply(n, g);
        let l = p.add_load(n, Load::new(2000.0));
        run(&mut p, 0.2);
        assert!(p.loads[l].receiving_power > 0.0);
        run(&mut p, 10.0);
        assert!((p.loads[l].receiving_power - 2000.0).abs() < 1.0);
        assert!(p.supplies[g].current_supply > 1990.0);
    }

    #[test]
    fn an_overloaded_network_shares_power_in_proportion() {
        let mut p = PowerState::default();
        let n = p.add_network();
        p.add_supply(n, Supply::new(1000.0));
        let a = p.add_load(n, Load::new(1000.0));
        let b = p.add_load(n, Load::new(3000.0));
        run(&mut p, 5.0);
        assert!((p.loads[a].receiving_power - 250.0).abs() < 5.0);
        assert!((p.loads[b].receiving_power - 750.0).abs() < 15.0);
    }

    #[test]
    fn a_substation_and_an_apc_pass_power_down_the_chain_and_a_charged_apc_survives_an_outage() {
        let mut p = PowerState::default();
        let (hv, mv, lv) = (p.add_network(), p.add_network(), p.add_network());
        let gen_ = p.add_supply(hv, Supply::new(10_000.0));
        let mut sub = Battery::new(2_500_000.0, 0.0);
        sub.max_charge_rate = 150_000.0;
        sub.max_supply = 150_000.0;
        sub.supply_ramp_tolerance = 5000.0;
        sub.supply_ramp_rate = 1000.0;
        p.add_battery(hv, mv, sub);
        let mut apc = Battery::new(50_000.0, 50_000.0);
        apc.max_charge_rate = 5000.0;
        apc.max_supply = 10_000.0;
        apc.supply_ramp_tolerance = 1000.0;
        apc.supply_ramp_rate = 500.0;
        let apc_id = p.add_battery(mv, lv, apc);
        let lamp = p.add_load(lv, Load::new(500.0));
        run(&mut p, 60.0);
        assert!(p.loads[lamp].receiving_power > 490.0, "{}", p.loads[lamp].receiving_power);
        assert!(p.supplies[gen_].current_supply > 100.0, "генератор грузится");
        // Авария: генератор выключен, подстанция пуста — АПЦ держится на своей батарее.
        p.supplies[gen_].enabled = false;
        p.batteries[0].current_storage = 0.0;
        run(&mut p, 20.0);
        assert!(p.loads[lamp].receiving_power > 400.0, "работает от заряда АПЦ");
        let left = p.batteries[apc_id].current_storage;
        assert!(left < 50_000.0, "заряд АПЦ расходуется: {left}");
        // Пока батарея не сядет совсем.
        run(&mut p, 400.0);
        assert!(p.loads[lamp].receiving_power < 1.0, "свет погас");
    }

    #[test]
    fn an_smes_with_separate_input_and_output_buffers_a_generator() {
        // СМЭС: вход (клеммы) и выход — две сети.
        let mut p = PowerState::default();
        let (input, output) = (p.add_network(), p.add_network());
        p.add_supply(input, Supply::new(2000.0));
        let mut smes = Battery::new(1_000_000.0, 0.0);
        smes.max_charge_rate = 150_000.0;
        smes.max_supply = 150_000.0;
        let s = p.add_battery(input, output, smes);
        run(&mut p, 30.0);
        assert!(p.batteries[s].current_storage > 10_000.0, "заряжается от генератора");
        let load = p.add_load(output, Load::new(1500.0));
        run(&mut p, 10.0);
        assert!(p.loads[load].receiving_power > 1400.0, "СМЭС питает выход");
    }
}
