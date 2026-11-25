//! Консоль на сервере: разбор строки, проверка прав по флагу команды, выполнение, ответ игроку.
//! Права и список команд — в `admin.rs`; здесь то, что команды делают с игрой.

use astrocore_api::HookContext;
use astrocore_core::NetworkEntityId;
use astrocore_math::glam::Vec3;

use crate::admin::{self, ConsoleLine};
use crate::module::State;
use crate::systems::{GameSystem, maps};

/// Сколько предметов за раз можно создать командой `spawn`.
const SPAWN_MAX: u32 = 20;

/// Система консоли и прав: команда `console.exec`, ответы `build.console`, права `build.admin`.
#[derive(Default)]
pub struct ConsoleSystem {
    pub(crate) admin: admin::AdminState,
}

impl GameSystem for ConsoleSystem {
    fn id(&self) -> &'static str {
        "console"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[admin::CMD_CONSOLE]
    }

    fn command(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        _kind: &str,
        data: &[u8],
    ) {
        self.exec(st, ctx, actor, data);
    }

    fn tick(&mut self, st: &mut State, ctx: &mut HookContext<'_>, _dt: f32) {
        self.publish(st, ctx);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn token_of(st: &State, who: NetworkEntityId) -> Option<&str> {
    st.token_of(who)
}

impl ConsoleSystem {
    /// Строка ответа игроку; уходит клиенту компонентом `build.console` в конце тика.
    pub(crate) fn console_reply(&mut self, who: NetworkEntityId, error: bool, text: &str) {
        for part in text.lines() {
            let id = self.admin.next_id;
            self.admin.next_id += 1;
            let log = self.admin.log.entry(who).or_default();
            log.push_back(ConsoleLine {
                id,
                error,
                text: part.chars().take(admin::MAX_LINE).collect(),
            });
            while log.len() > admin::KEEP_LINES {
                log.pop_front();
            }
        }
        self.admin.dirty.insert(who);
    }

    /// Команда `console.exec`: одна строка консоли.
    fn exec(
        &mut self,
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        data: &[u8],
    ) {
        let Some(line) = std::str::from_utf8(data).ok().and_then(admin::clean) else {
            return;
        };
        let last = self.admin.last.get(&actor).copied().unwrap_or(f32::MIN);
        if st.clock - last < admin::MIN_GAP_S {
            return;
        }
        self.admin.last.insert(actor, st.clock);
        // Журнал сервера: кто и что ввёл (как лог админских действий в SS14).
        println!("консоль {}: {line}", actor.0);
        self.console_reply(actor, false, &format!("> {line}"));
        let words = admin::tokenize(&line);
        let Some((name, args)) = words.split_first() else {
            return;
        };
        let Some(spec) = admin::spec(name) else {
            self.console_reply(
                actor,
                true,
                &format!("Неизвестная команда «{name}». Введите help."),
            );
            return;
        };
        if !st.admins.allows(token_of(st, actor), spec.flag) {
            self.console_reply(
                actor,
                true,
                &format!(
                    "Нет прав на «{}»: нужен флаг {}.",
                    spec.name,
                    spec.flag.unwrap_or("")
                ),
            );
            return;
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = match spec.name {
            "help" => Ok(self.cmd_help(st, actor, &args)),
            "whoami" => Ok(self.cmd_whoami(st, actor)),
            "list" => Ok(Self::cmd_list(st, ctx)),
            "aghost" => Self::cmd_aghost(st, ctx, actor),
            "tp" => Self::cmd_tp(ctx, actor, &args),
            "spawn" => Self::cmd_spawn(st, ctx, actor, &args),
            "lsmap" => Ok(maps::lsmap(ctx)),
            "mapping" => maps::mapping(st, ctx, actor, &args),
            "loadmap" => maps::loadmap(st, ctx, &args),
            "savemap" => maps::savemap(st, ctx, &args),
            "rmmap" => maps::rmmap(st, ctx, &args),
            "pausemap" => maps::set_flags(ctx, &args, Some(true), false),
            "unpausemap" => maps::set_flags(ctx, &args, Some(false), false),
            "querymappaused" => maps::querymappaused(ctx, &args),
            "mapinit" => maps::set_flags(ctx, &args, None, true),
            "endround" => Self::cmd_endround(st),
            _ => Err(format!("Команда «{}» не реализована.", spec.name)),
        };
        match result {
            Ok(text) if text.is_empty() => {}
            Ok(text) => self.console_reply(actor, false, &text),
            Err(text) => self.console_reply(actor, true, &text),
        }
    }

    fn cmd_help(&self, st: &State, actor: NetworkEntityId, args: &[&str]) -> String {
        let token = token_of(st, actor);
        if let Some(name) = args.first() {
            return match admin::spec(name) {
                Some(c) if st.admins.allows(token, c.flag) => {
                    format!("{}\n{}", c.usage, c.help)
                }
                _ => format!("Нет команды «{name}»."),
            };
        }
        admin::COMMANDS
            .iter()
            .filter(|c| st.admins.allows(token, c.flag))
            .map(|c| format!("{} — {}", c.name, c.help))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn cmd_whoami(&self, st: &State, actor: NetworkEntityId) -> String {
        match st.admins.rank_of(token_of(st, actor)) {
            Some(r) => format!(
                "Номер {}. Ранг: {}. Флаги: {}.",
                actor.0,
                r.title,
                r.flags.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
            None => format!("Номер {}. Вы не админ.", actor.0),
        }
    }

    fn cmd_list(st: &State, ctx: &HookContext<'_>) -> String {
        ctx.all_players()
            .map(|p| {
                let a = st.actors.get(&p);
                let state = match a {
                    Some(a) if a.admin_ghost => "админ-призрак",
                    Some(a) if a.ghost => "призрак",
                    Some(_) => "жив",
                    None => "в лобби",
                };
                let job = st.player_job.get(&p).map_or("—", String::as_str);
                let name = a.map_or("", |a| a.slots.current().name.as_str());
                format!("{} | {job} | {state} | {name}", p.0)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Админ-призрак: летает, пока не вернётся в тело; в лобби — сразу выходит наблюдателем.
    fn cmd_aghost(
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
    ) -> Result<String, String> {
        if st
            .lobby
            .as_ref()
            .is_some_and(|l| l.in_lobby.contains(&actor))
        {
            st.lobby_observe(ctx, actor);
            st.actor(actor).admin_ghost = true;
            return Ok("Вы админ-призрак.".into());
        }
        let clock = st.clock;
        let a = st.actor(actor);
        if a.admin_ghost {
            a.admin_ghost = false;
            a.ghost = false;
            ctx.set_ghost(actor, false);
            st.publish(ctx, actor);
            return Ok("Вы снова в теле.".into());
        }
        if a.ghost {
            return Err("Вы уже призрак: aghost работает из живого тела или лобби.".into());
        }
        a.ghost = true;
        a.admin_ghost = true;
        a.respawn_at = clock;
        ctx.set_ghost(actor, true);
        st.publish(ctx, actor);
        Ok("Вы админ-призрак. Ещё раз aghost — вернуться.".into())
    }

    fn cmd_tp(
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        args: &[&str],
    ) -> Result<String, String> {
        let here = ctx.position_of(actor).ok_or("Нет вашего положения.")?;
        let to = match args {
            [x, z] => {
                let (x, z): (f32, f32) = (
                    x.parse().map_err(|_| format!("«{x}» — не число."))?,
                    z.parse().map_err(|_| format!("«{z}» — не число."))?,
                );
                Vec3::new(x, here.y, z)
            }
            [x, z, m] => {
                let (x, z): (f32, f32) = (
                    x.parse().map_err(|_| format!("«{x}» — не число."))?,
                    z.parse().map_err(|_| format!("«{z}» — не число."))?,
                );
                let map = maps::parse_map(m)?;
                if ctx.map_def(map).is_none() {
                    return Err(format!("Карты {} нет.", map.0));
                }
                // На другую карту — переносом (клиент загружает её мир), на свою — телепортом.
                let to = Vec3::new(x, ::maps::FLOOR_TOP_M + 0.05, z);
                if ctx.map_of(actor) != Some(map) {
                    ctx.move_to_map(actor, map, to);
                    return Ok(format!("Вы на карте {} в ({x:.1}, {z:.1}).", map.0));
                }
                to
            }
            [who] => {
                let id: u32 = who
                    .parse()
                    .map_err(|_| format!("«{who}» — не номер игрока."))?;
                let target = NetworkEntityId(id);
                let at = ctx
                    .position_of(target)
                    .ok_or(format!("Нет игрока с номером {id}."))?;
                // Игрок на другой карте: перенос на его карту.
                if let Some(map) = ctx.map_of(target).filter(|m| ctx.map_of(actor) != Some(*m)) {
                    ctx.move_to_map(actor, map, at);
                    return Ok(format!("Вы у игрока {id} на карте {}.", map.0));
                }
                at
            }
            _ => return Err("Использование: tp <x> <z> [карта] | tp <номер игрока>".into()),
        };
        ctx.teleport(actor, to);
        Ok(format!("Вы в точке ({:.1}, {:.1}).", to.x, to.z))
    }

    fn cmd_spawn(
        st: &mut State,
        ctx: &mut HookContext<'_>,
        actor: NetworkEntityId,
        args: &[&str],
    ) -> Result<String, String> {
        let (name, count) = match args {
            [name] => (*name, 1u32),
            [name, n] => (*name, n.parse().map_err(|_| format!("«{n}» — не число."))?),
            _ => return Err("Использование: spawn <прототип> [число]".into()),
        };
        if !(1..=SPAWN_MAX).contains(&count) {
            return Err(format!("Число от 1 до {SPAWN_MAX}."));
        }
        let at = ctx.position_of(actor).ok_or("Нет вашего положения.")?;
        let mut made = 0;
        for _ in 0..count {
            let Some(item) = st.create_item(name) else {
                return Err(format!("Нет предмета «{name}»."));
            };
            if st.place(ctx, &item, at) {
                made += 1;
            }
        }
        Ok(format!("Создано {made} × {name}."))
    }

    fn cmd_endround(st: &State) -> Result<String, String> {
        let signal = st
            .round
            .signal
            .as_ref()
            .ok_or("Сервер не умеет перезапускать раунд.")?;
        signal.request_restart();
        Ok("Раунд заканчивается.".into())
    }

    /// Разослать права (когда изменились) и ответы консоли (когда есть новые строки).
    fn publish(&mut self, st: &State, ctx: &mut HookContext<'_>) {
        let ids: Vec<NetworkEntityId> = ctx.all_players().collect();
        self.admin.log.retain(|p, _| ids.contains(p));
        self.admin.sent.retain(|p, _| ids.contains(p));
        self.admin.last.retain(|p, _| ids.contains(p));
        for p in &ids {
            let bytes = admin::encode_admin(&admin::view_for(&st.admins, token_of(st, *p)));
            if self.admin.sent.get(p) != Some(&bytes) {
                ctx.set_component(*p, admin::ADMIN_COMPONENT, bytes.clone());
                self.admin.sent.insert(*p, bytes);
            }
        }
        for p in std::mem::take(&mut self.admin.dirty) {
            let Some(lines) = self.admin.log.get(&p) else {
                continue;
            };
            let mut view = admin::ConsoleView {
                lines: lines.iter().cloned().collect(),
            };
            let mut bytes = admin::encode_console(&view);
            while bytes.len() > admin::COMPONENT_MAX_BYTES && !view.lines.is_empty() {
                view.lines.remove(0);
                bytes = admin::encode_console(&view);
            }
            ctx.set_component(p, admin::CONSOLE_COMPONENT, bytes);
        }
    }
}
