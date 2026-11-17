//! Редактор персонажа в лобби (`HumanoidProfileEditor` SS14): имя, возраст, пол, цвет кожи, глаза,
//! причёска и борода, одежда, рюкзак, описание и превью с поворотом.
//!
//! Окно рисуется только по правилам, которые прислал сервер (`gameplay::character::Rules`):
//! пределы, виды и стили берутся оттуда, а не из кода клиента. Правки лежат в черновике и уходят
//! серверу командой `char.save` по кнопке «Сохранить»; сервер проверяет профиль и присылает
//! принятый вариант назад (`build.profile`).

use bevy_egui::egui::{self, Color32};
use gameplay::character::{
    Backpack, CMD_CHAR_CREATE, CMD_CHAR_DELETE, CMD_CHAR_RANDOM, CMD_CHAR_SAVE, CMD_CHAR_SELECT,
    Clothing, Gender, Priority, Profile, Rules, Sex, SlotsView, Style, Unavailable, encode_profile,
    skin_color,
};

use crate::hud::Commands;

/// Ширина колонки превью и формы, пиксели.
const PREVIEW_WIDTH: f32 = 140.0;
const FORM_WIDTH: f32 = 460.0;

/// Вкладки редактора.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    #[default]
    Look,
    Jobs,
}

/// Приоритет должности в профиле: `None` — «никогда» (записи нет). «Высокий» всегда один: прежний
/// становится «средним», как в `EnsureValid` SS14.
pub fn set_priority(p: &mut Profile, job: &str, priority: Option<Priority>) {
    match priority {
        None => {
            p.job_priorities.remove(job);
        }
        Some(Priority::High) => {
            for v in p.job_priorities.values_mut() {
                if *v == Priority::High {
                    *v = Priority::Medium;
                }
            }
            p.job_priorities.insert(job.to_string(), Priority::High);
        }
        Some(pr) => {
            p.job_priorities.insert(job.to_string(), pr);
        }
    }
}

fn priority_label(p: Option<Priority>) -> &'static str {
    match p {
        Some(Priority::High) => "Высокий",
        Some(Priority::Medium) => "Средний",
        Some(Priority::Low) => "Низкий",
        None => "Никогда",
    }
}

/// Вкладка «Должности»: по отделам, у каждой должности четыре приоритета, внизу — что делать, если
/// ничего не подошло.
fn jobs_tab(ui: &mut egui::Ui, p: &mut Profile, rules: &Rules) {
    egui::ScrollArea::vertical()
        .max_height(420.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.vertical(|ui| {
                for dept in &rules.departments {
                    let jobs: Vec<_> = rules.jobs.iter().filter(|j| j.dept == dept.id).collect();
                    if jobs.is_empty() {
                        continue;
                    }
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(&dept.name)
                            .strong()
                            .color(Color32::from_rgb(
                                dept.color[0],
                                dept.color[1],
                                dept.color[2],
                            )),
                    );
                    for job in jobs {
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(170.0, 18.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| ui.label(&job.title),
                            );
                            let now = p.job_priorities.get(&job.id).copied();
                            for choice in [
                                Some(Priority::High),
                                Some(Priority::Medium),
                                Some(Priority::Low),
                                None,
                            ] {
                                if ui
                                    .selectable_label(now == choice, priority_label(choice))
                                    .clicked()
                                {
                                    set_priority(p, &job.id, choice);
                                }
                            }
                        });
                    }
                }
                ui.separator();
                ui.label("Если не досталось желаемой должности:");
                ui.radio_value(
                    &mut p.unavailable,
                    Unavailable::StayInLobby,
                    "Остаться в лобби",
                );
                ui.radio_value(
                    &mut p.unavailable,
                    Unavailable::SpawnAsOverflow,
                    "Появиться на любой свободной (пассажиром)",
                );
            });
        });
}

/// Состояние окна редактора.
#[derive(Default)]
pub struct CharacterEditor {
    pub open: bool,
    /// Черновик; `None`, пока не пришёл профиль от сервера.
    pub draft: Option<Profile>,
    /// Профиль, который сервер принял в последний раз.
    server: Option<Profile>,
    hair_search: String,
    facial_search: String,
    /// Сторона превью: 0 — юг (лицом), 1 — север, 2 — восток, 3 — запад.
    pub preview_side: u8,
    /// Вкладка: внешность или должности.
    pub tab: Tab,
    /// Отладочный автоплей уже отправил свои команды.
    pub autoplay_done: bool,
    /// Текстура превью и то, для чего она построена.
    preview: Option<((gameplay::character::Look, u8), egui::TextureHandle)>,
}

fn sex_label(s: Sex) -> &'static str {
    match s {
        Sex::Male => "Мужской",
        Sex::Female => "Женский",
        Sex::Unsexed => "Бесполый",
    }
}

fn gender_label(g: Gender) -> &'static str {
    match g {
        Gender::Male => "он",
        Gender::Female => "она",
        Gender::Epicene => "они",
        Gender::Neuter => "оно",
    }
}

impl CharacterEditor {
    /// Сервер прислал профиль: если он новый, черновик заменяется им.
    pub fn sync(&mut self, from_server: Option<&Profile>) {
        let Some(p) = from_server else { return };
        if self.server.as_ref() != Some(p) {
            self.server = Some(p.clone());
            self.draft = Some(p.clone());
        }
    }

    /// Текстура превью черновика с выбранной стороны: лист собирает `outfits` (тело, цвет кожи,
    /// глаза, причёска, борода), одежды в превью нет. Перестраивается, только когда меняется вид.
    pub fn preview(
        &mut self,
        ctx: &egui::Context,
        outfits: &mut crate::outfit::Outfits,
        rules: &Rules,
    ) -> Option<egui::TextureId> {
        if !self.open {
            return None;
        }
        let look = self.draft.as_ref()?.validated(rules).look(rules);
        let key = (look, self.preview_side);
        if self.preview.as_ref().map(|(k, _)| k) != Some(&key) {
            let sheet = outfits.preview_sheet(&key.0);
            let cell = sheet.height();
            let side = u32::from(key.1).min(3);
            let img = image::imageops::crop_imm(&sheet, side * cell, 0, cell, cell).to_image();
            let color = egui::ColorImage::from_rgba_unmultiplied(
                [cell as usize, cell as usize],
                img.as_raw(),
            );
            let tex = ctx.load_texture("character_preview", color, egui::TextureOptions::NEAREST);
            self.preview = Some((key, tex));
        }
        self.preview.as_ref().map(|(_, t)| t.id())
    }

    /// Есть правки, которых сервер ещё не видел.
    pub fn dirty(&self) -> bool {
        self.draft.is_some() && self.draft != self.server
    }

    /// Команда сохранения; `None`, если сохранять нечего.
    pub fn save_command(&self) -> Option<(&'static str, Vec<u8>)> {
        let draft = self.draft.as_ref().filter(|_| self.dirty())?;
        Some((CMD_CHAR_SAVE, encode_profile(draft)))
    }

    /// Откатить черновик к принятому сервером профилю.
    pub fn reset(&mut self) {
        self.draft.clone_from(&self.server);
    }
}

/// Какие кнопки слотов доступны: (создать, удалить).
fn slot_buttons(v: &SlotsView) -> (bool, bool) {
    (v.names.len() < v.max, v.names.len() > 1)
}

/// Вкладки слотов: выбрать, создать (пока не достигнут предел), удалить выбранный (пока он не
/// единственный).
fn slot_tabs(ui: &mut egui::Ui, v: &SlotsView) -> Commands {
    let mut out: Commands = Vec::new();
    ui.horizontal_wrapped(|ui| {
        for (i, name) in v.names.iter().enumerate() {
            let label = format!("{}: {}", i + 1, name);
            if ui.selectable_label(i == v.selected, label).clicked() && i != v.selected {
                out.push((CMD_CHAR_SELECT, vec![i as u8]));
            }
        }
        let (can_create, can_delete) = slot_buttons(v);
        if can_create && ui.button("+").on_hover_text("Новый персонаж").clicked() {
            out.push((CMD_CHAR_CREATE, Vec::new()));
        }
        if can_delete
            && ui
                .button("Удалить")
                .on_hover_text("Удалить выбранного персонажа")
                .clicked()
        {
            out.push((CMD_CHAR_DELETE, vec![v.selected as u8]));
        }
    });
    out
}

/// Список стилей с поиском; `current` — выбранный идентификатор (пусто — без стиля).
fn style_picker(
    ui: &mut egui::Ui,
    label: &str,
    styles: &[Style],
    search: &mut String,
    current: &mut String,
    color: &mut [u8; 3],
) {
    let shown = styles
        .iter()
        .find(|s| s.id == *current)
        .map_or("нет", |s| s.name.as_str())
        .to_string();
    egui::CollapsingHeader::new(format!("{label}: {shown}"))
        .id_salt(label)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Цвет");
                ui.color_edit_button_srgb(color);
                ui.add(egui::TextEdit::singleline(search).hint_text("поиск"));
            });
            let needle = search.to_lowercase();
            egui::ScrollArea::vertical()
                .id_salt(format!("{label}_list"))
                .max_height(150.0)
                .show(ui, |ui| {
                    if ui.selectable_label(current.is_empty(), "нет").clicked() {
                        current.clear();
                    }
                    for s in styles
                        .iter()
                        .filter(|s| needle.is_empty() || s.name.to_lowercase().contains(&needle))
                    {
                        if ui.selectable_label(*current == s.id, &s.name).clicked() {
                            current.clone_from(&s.id);
                        }
                    }
                });
        });
}

/// Окно редактора. `preview` — текстура превью выбранной стороны (рисует вызывающий, у него
/// есть тело и слои). Возвращает команды для сервера.
pub fn character_window(
    ctx: &egui::Context,
    editor: &mut CharacterEditor,
    rules: &Rules,
    slots: Option<&SlotsView>,
    preview: Option<egui::TextureId>,
) -> Commands {
    let mut out: Commands = Vec::new();
    if !editor.open {
        return out;
    }
    let mut open = true;
    let CharacterEditor {
        draft,
        hair_search,
        facial_search,
        preview_side,
        tab,
        ..
    } = editor;
    let mut want_save = false;
    let mut want_reset = false;
    let mut want_random = false;
    let mut slot_commands: Commands = Vec::new();
    egui::Window::new("Персонаж")
        // Экран лобби лежит на слое «Передний план» и закрыл бы обычное окно.
        .order(egui::Order::Foreground)
        .open(&mut open)
        .default_width(PREVIEW_WIDTH + FORM_WIDTH + 24.0)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let Some(p) = draft.as_mut() else {
                ui.label("Профиль загружается…");
                return;
            };
            if let Some(v) = slots {
                slot_commands = slot_tabs(ui, v);
                ui.separator();
            }
            ui.horizontal(|ui| {
                ui.selectable_value(tab, Tab::Look, "Внешность");
                ui.selectable_value(tab, Tab::Jobs, "Должности");
            });
            if *tab == Tab::Jobs {
                jobs_tab(ui, p, rules);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Сохранить").clicked() {
                        want_save = true;
                    }
                    if ui.button("Сбросить").clicked() {
                        want_reset = true;
                    }
                });
                return;
            }
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(PREVIEW_WIDTH);
                    let size = egui::vec2(128.0, 128.0);
                    match preview {
                        Some(tex) => {
                            ui.image((tex, size));
                        }
                        None => {
                            ui.allocate_space(size);
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("◀").clicked() {
                            *preview_side = (*preview_side + 3) % 4;
                        }
                        if ui.button("▶").clicked() {
                            *preview_side = (*preview_side + 1) % 4;
                        }
                    });
                    if ui.button("Случайный").clicked() {
                        want_random = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .max_width(FORM_WIDTH)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(FORM_WIDTH - 20.0);
                            ui.horizontal(|ui| {
                                ui.label("Имя");
                                ui.add(
                                    egui::TextEdit::singleline(&mut p.name)
                                        .desired_width(240.0)
                                        .char_limit(rules.limits.max_name_len),
                                );
                            });
                            let sp = rules.species_of(&p.species).clone();
                            ui.horizontal(|ui| {
                                ui.label("Вид");
                                egui::ComboBox::from_id_salt("species")
                                    .selected_text(&sp.name)
                                    .show_ui(ui, |ui| {
                                        for s in rules.species.iter().filter(|s| s.round_start) {
                                            if ui
                                                .selectable_label(s.id == p.species, &s.name)
                                                .clicked()
                                            {
                                                p.species.clone_from(&s.id);
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("Возраст");
                                ui.add(egui::Slider::new(&mut p.age, sp.min_age..=sp.max_age));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Пол");
                                for &s in &sp.sexes {
                                    if ui.radio_value(&mut p.sex, s, sex_label(s)).clicked() {
                                        p.gender = s.default_gender();
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Местоимения");
                                for g in [
                                    Gender::Male,
                                    Gender::Female,
                                    Gender::Epicene,
                                    Gender::Neuter,
                                ] {
                                    ui.radio_value(&mut p.gender, g, gender_label(g));
                                }
                            });
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("Кожа");
                                ui.add(egui::Slider::new(&mut p.appearance.skin_tone, 0.0..=100.0));
                                let c = skin_color(p.appearance.skin_tone);
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(24.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    3.0,
                                    Color32::from_rgb(c[0], c[1], c[2]),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Глаза");
                                ui.color_edit_button_srgb(&mut p.appearance.eyes);
                            });
                            style_picker(
                                ui,
                                "Причёска",
                                &rules.catalog.hair,
                                hair_search,
                                &mut p.appearance.hair,
                                &mut p.appearance.hair_color,
                            );
                            style_picker(
                                ui,
                                "Борода",
                                &rules.catalog.facial_hair,
                                facial_search,
                                &mut p.appearance.facial_hair,
                                &mut p.appearance.facial_hair_color,
                            );
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("Одежда");
                                ui.radio_value(&mut p.clothing, Clothing::Jumpsuit, "Комбинезон");
                                ui.radio_value(&mut p.clothing, Clothing::Jumpskirt, "Юбка");
                            });
                            ui.horizontal(|ui| {
                                ui.label("Рюкзак");
                                ui.radio_value(&mut p.backpack, Backpack::Backpack, "Рюкзак");
                                ui.radio_value(&mut p.backpack, Backpack::Satchel, "Сумка");
                                ui.radio_value(&mut p.backpack, Backpack::Duffel, "Вещмешок");
                            });
                            ui.label("Описание");
                            ui.add(
                                egui::TextEdit::multiline(&mut p.flavor_text)
                                    .desired_rows(3)
                                    .desired_width(FORM_WIDTH - 20.0)
                                    .char_limit(rules.limits.max_flavor_len),
                            );
                        });
                    });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Сохранить").clicked() {
                    want_save = true;
                }
                if ui.button("Сбросить").clicked() {
                    want_reset = true;
                }
            });
        });
    if !open {
        editor.open = false;
    }
    out.extend(slot_commands);
    if want_random {
        out.push((CMD_CHAR_RANDOM, Vec::new()));
    }
    if want_reset {
        editor.reset();
    }
    if want_save && let Some(cmd) = editor.save_command() {
        out.push(cmd);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        let mut r = Rules::default();
        r.catalog.hair = vec![
            Style {
                id: "HumanHairAfro".into(),
                name: "afro".into(),
                png: "Mobs/Customization/human_hair.rsi/afro.png".into(),
            },
            Style {
                id: "HumanHairBob".into(),
                name: "bob".into(),
                png: "Mobs/Customization/human_hair.rsi/bob.png".into(),
            },
        ];
        r
    }

    fn frame(ctx: &egui::Context, f: impl FnOnce(&egui::Context)) -> usize {
        let mut f = Some(f);
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                f(ui.ctx());
            }
        });
        out.textures_delta.clear();
        out.shapes.len()
    }

    #[test]
    fn setting_a_high_priority_demotes_the_previous_one_and_never_removes_the_entry() {
        let mut p = Profile::default();
        set_priority(&mut p, "Captain", Some(Priority::High));
        set_priority(&mut p, "Chef", Some(Priority::Low));
        set_priority(&mut p, "Janitor", Some(Priority::High));
        assert_eq!(p.job_priorities["Janitor"], Priority::High);
        assert_eq!(
            p.job_priorities["Captain"],
            Priority::Medium,
            "прежний «высокий» ослаблен"
        );
        assert_eq!(p.job_priorities["Chef"], Priority::Low);
        set_priority(&mut p, "Chef", None);
        assert!(
            !p.job_priorities.contains_key("Chef"),
            "«никогда» — записи нет"
        );
    }

    #[test]
    fn a_draft_is_saved_only_when_it_differs_and_reset_returns_to_the_server_profile() {
        let mut e = CharacterEditor::default();
        assert!(e.save_command().is_none(), "профиля ещё нет");
        let server = Profile::default();
        e.sync(Some(&server));
        assert!(!e.dirty() && e.save_command().is_none(), "нечего сохранять");
        e.draft.as_mut().unwrap().name = "Анна".into();
        assert!(e.dirty());
        let (kind, data) = e.save_command().expect("есть правки");
        assert_eq!(kind, CMD_CHAR_SAVE);
        assert_eq!(
            gameplay::character::decode_profile(&data).unwrap().name,
            "Анна"
        );
        e.reset();
        assert!(!e.dirty());
        // Сервер прислал другой профиль (например, случайный): черновик следует за ним.
        let mut other = server;
        other.age = 44;
        e.sync(Some(&other));
        assert_eq!(e.draft.as_ref().unwrap().age, 44);
    }

    #[test]
    fn slot_tabs_offer_select_create_and_delete_within_the_limits() {
        let ctx = egui::Context::default();
        let one = SlotsView {
            selected: 0,
            names: vec!["Иван".into()],
            max: 3,
        };
        let mut commands = Commands::new();
        frame(&ctx, |c| {
            egui::Area::new(egui::Id::new("t")).show(c, |ui| commands = slot_tabs(ui, &one));
        });
        assert!(commands.is_empty(), "без кликов команд нет");
        let view = |n: usize, max: usize| SlotsView {
            selected: 0,
            names: (0..n).map(|i| format!("n{i}")).collect(),
            max,
        };
        assert_eq!(
            slot_buttons(&view(1, 5)),
            (true, false),
            "единственного не удалить"
        );
        assert_eq!(slot_buttons(&view(3, 5)), (true, true));
        assert_eq!(
            slot_buttons(&view(5, 5)),
            (false, true),
            "на пределе нового нет"
        );
    }

    #[test]
    fn the_window_draws_from_the_rules_and_stays_quiet_without_input() {
        let ctx = egui::Context::default();
        let mut e = CharacterEditor {
            open: true,
            ..Default::default()
        };
        let r = rules();
        // Без профиля — только надпись.
        let shapes = frame(&ctx, |c| {
            assert!(character_window(c, &mut e, &r, None, None).is_empty());
        });
        assert!(shapes > 0);
        e.sync(Some(&Profile::default()));
        let shapes = frame(&ctx, |c| {
            assert!(
                character_window(c, &mut e, &r, None, None).is_empty(),
                "без кликов команд нет"
            );
        });
        assert!(shapes > 10, "поля редактора нарисованы: {shapes}");
        e.open = false;
        assert!(character_window(&ctx, &mut e, &r, None, None).is_empty());
    }
}
