//! Консоль (`DebugConsole` SS14): окно сверху экрана по клавише `` ` ``. Ответы приходят от
//! сервера компонентом `build.console`, подсказки и дополнение — по списку команд из `build.admin`
//! (там только то, что разрешено рангу игрока; сервер проверяет права сам).

use bevy_egui::egui::{self, Color32, Key};
use gameplay::admin::{AdminView, ConsoleView};

use crate::hud::Commands;

/// Сколько введённых строк помнит консоль.
const HISTORY: usize = 50;

#[derive(Default)]
pub struct ConsoleState {
    pub open: bool,
    input: String,
    history: Vec<String>,
    /// Какая строка истории показана в поле ввода (с конца).
    from_end: Option<usize>,
    /// Вернуть полю ввода фокус на следующем кадре.
    focus: bool,
}

/// Команды, начало которых совпало с введённым первым словом (пока слово одно).
pub fn suggestions<'a>(
    input: &str,
    admin: Option<&'a AdminView>,
) -> Vec<&'a gameplay::admin::CommandInfo> {
    let word = input.trim_start();
    if word.contains(char::is_whitespace) {
        return Vec::new();
    }
    admin.map_or_else(Vec::new, |a| {
        a.commands
            .iter()
            .filter(|c| c.name.starts_with(word))
            .collect()
    })
}

impl ConsoleState {
    fn remember(&mut self, line: &str) {
        if self.history.last().map(String::as_str) != Some(line) {
            self.history.push(line.to_string());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
        }
        self.from_end = None;
    }

    /// Листать историю: `up` — к более старым.
    fn browse(&mut self, up: bool) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.from_end, up) {
            (None, true) => Some(0),
            (Some(i), true) => Some((i + 1).min(self.history.len() - 1)),
            (Some(0) | None, false) => None,
            (Some(i), false) => Some(i - 1),
        };
        self.from_end = next;
        self.input = next.map_or_else(String::new, |i| {
            self.history[self.history.len() - 1 - i].clone()
        });
    }
}

/// Окно консоли. Возвращает команды для сервера.
pub fn console_panel(
    ctx: &egui::Context,
    state: &mut ConsoleState,
    view: Option<&ConsoleView>,
    admin: Option<&AdminView>,
) -> Commands {
    let mut out: Commands = Vec::new();
    if ctx.input(|i| i.key_pressed(Key::Backtick)) {
        state.open = !state.open;
        state.focus = state.open;
        state.input.retain(|c| c != '`');
    }
    if !state.open {
        return out;
    }
    let width = ctx.content_rect().width();
    egui::Area::new(egui::Id::new("console"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(0.0, 0.0))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(6, 8, 14, 235))
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    let rank = admin
                        .and_then(|a| a.rank.as_deref())
                        .map_or("не админ".to_string(), str::to_string);
                    ui.label(
                        egui::RichText::new(format!("Консоль · {rank} · ` — закрыть"))
                            .color(Color32::from_gray(150))
                            .size(12.0),
                    );
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            for l in view.map_or(&[][..], |v| &v.lines[..]) {
                                let color = if l.error {
                                    Color32::from_rgb(240, 110, 110)
                                } else if l.text.starts_with("> ") {
                                    Color32::from_rgb(130, 190, 255)
                                } else {
                                    Color32::from_gray(225)
                                };
                                ui.label(
                                    egui::RichText::new(&l.text)
                                        .monospace()
                                        .color(color)
                                        .size(13.0),
                                );
                            }
                        });
                    let hints = suggestions(&state.input, admin);
                    if !state.input.trim().is_empty() && !hints.is_empty() && hints.len() <= 8 {
                        ui.label(
                            egui::RichText::new(
                                hints
                                    .iter()
                                    .map(|c| c.usage.as_str())
                                    .collect::<Vec<_>>()
                                    .join("   "),
                            )
                            .monospace()
                            .color(Color32::from_rgb(190, 190, 120))
                            .size(12.0),
                        );
                    }
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut state.input)
                            .hint_text("help — список команд; Tab — дополнить; стрелки вверх/вниз — история")
                            .font(egui::TextStyle::Monospace)
                            .char_limit(gameplay::admin::MAX_LINE)
                            .lock_focus(true)
                            .desired_width(f32::INFINITY),
                    );
                    if state.focus {
                        edit.request_focus();
                        state.focus = false;
                    }
                    if edit.has_focus() {
                        let (tab, up, down) = ui.input_mut(|i| {
                            (
                                i.consume_key(egui::Modifiers::NONE, Key::Tab),
                                i.key_pressed(Key::ArrowUp),
                                i.key_pressed(Key::ArrowDown),
                            )
                        });
                        if tab {
                            let hints = suggestions(&state.input, admin);
                            if let Some(first) = hints.first() {
                                state.input = format!("{} ", first.name);
                            }
                        }
                        if up || down {
                            state.browse(up);
                        }
                    }
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let line = std::mem::take(&mut state.input);
                        if let Some(line) = gameplay::admin::clean(&line) {
                            state.remember(&line);
                            out.push((gameplay::admin::CMD_CONSOLE, line.into_bytes()));
                        }
                        state.focus = true;
                    }
                });
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gameplay::admin::CommandInfo;

    fn admin() -> AdminView {
        let c = |name: &str| CommandInfo {
            name: name.into(),
            usage: format!("{name} <x>"),
            help: String::new(),
        };
        AdminView {
            rank: None,
            flags: Vec::new(),
            commands: vec![c("spawn"), c("help"), c("setjob")],
        }
    }

    #[test]
    fn suggestions_match_the_first_word_only() {
        let a = admin();
        let names = |s: &str| -> Vec<&str> {
            suggestions(s, Some(&a))
                .iter()
                .map(|c| c.name.as_str())
                .collect()
        };
        assert_eq!(names("s"), ["spawn", "setjob"]);
        assert_eq!(names("sp"), ["spawn"]);
        assert!(
            names("spawn Cro").is_empty(),
            "после первого слова подсказок нет"
        );
        assert!(suggestions("s", None).is_empty());
    }

    #[test]
    fn history_walks_back_and_forth_and_skips_repeats() {
        let mut s = ConsoleState::default();
        s.remember("help");
        s.remember("spawn Crowbar");
        s.remember("spawn Crowbar");
        assert_eq!(s.history.len(), 2);
        s.browse(true);
        assert_eq!(s.input, "spawn Crowbar");
        s.browse(true);
        assert_eq!(s.input, "help");
        s.browse(true);
        assert_eq!(s.input, "help", "дальше самой старой не идём");
        s.browse(false);
        assert_eq!(s.input, "spawn Crowbar");
        s.browse(false);
        assert_eq!(s.input, "", "вниз за новейшую — пустая строка");
    }

    /// Ответы сервера рисуются в окне консоли (а не теряются за пустой областью прокрутки).
    #[test]
    fn the_replies_are_drawn_in_the_open_console() {
        let ctx = egui::Context::default();
        let mut state = ConsoleState {
            open: true,
            ..Default::default()
        };
        let view = ConsoleView {
            lines: vec![
                gameplay::admin::ConsoleLine {
                    id: 0,
                    error: false,
                    text: "> whoami".into(),
                },
                gameplay::admin::ConsoleLine {
                    id: 1,
                    error: false,
                    text: "Номер 79. Вы не админ.".into(),
                },
            ],
        };
        let mut painted = String::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                console_panel(ui.ctx(), &mut state, Some(&view), None);
            });
            out.textures_delta.clear();
            painted.clear();
            for clipped in &out.shapes {
                collect_text(&clipped.shape, &mut painted);
            }
        }
        assert!(painted.contains("whoami"), "нарисовано: {painted}");
        assert!(painted.contains("Вы не админ"), "нарисовано: {painted}");
    }

    fn collect_text(shape: &egui::epaint::Shape, out: &mut String) {
        match shape {
            egui::epaint::Shape::Text(t) => {
                out.push_str(&t.galley.job.text);
                out.push('\n');
            }
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }
}
