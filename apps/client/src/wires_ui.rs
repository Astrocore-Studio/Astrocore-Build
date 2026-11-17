use bevy_egui::egui::{self, Color32, Rect, Vec2};

use crate::hud::{Commands, Icons};

fn wire_color(name: &str) -> Color32 {
    let [r, g, b] = match name {
        "Red" => [255, 0, 0],
        "Blue" => [0, 0, 255],
        "Green" => [0, 128, 0],
        "Orange" => [255, 165, 0],
        "Brown" => [165, 42, 42],
        "Gold" => [255, 215, 0],
        "Gray" => [128, 128, 128],
        "Cyan" => [0, 255, 255],
        "Navy" => [0, 0, 128],
        "Purple" => [128, 0, 128],
        "Pink" => [255, 192, 203],
        "Fuchsia" => [255, 0, 255],
        "Lime" => [0, 255, 0],
        "DeepSkyBlue" => [0, 191, 255],
        _ => [255, 255, 255],
    };
    Color32::from_rgb(r, g, b)
}

fn texture(
    ui: &egui::Ui,
    icons: &mut Icons,
    ctx: &egui::Context,
    name: &str,
    rect: Rect,
    tint: Color32,
) {
    wire_texture(ui, icons, ctx, name, rect, tint, [false, false]);
}

fn wire_texture(
    ui: &egui::Ui,
    icons: &mut Icons,
    ctx: &egui::Context,
    name: &str,
    rect: Rect,
    tint: Color32,
    flip: [bool; 2],
) {
    if let Some(image) = icons.load(
        ctx,
        &format!("Textures/Interface/WireHacking/{name}.svg.96dpi.png"),
    ) {
        ui.painter().image(
            image,
            rect,
            Rect::from_min_max(
                egui::pos2(f32::from(u8::from(flip[0])), f32::from(u8::from(flip[1]))),
                egui::pos2(f32::from(u8::from(!flip[0])), f32::from(u8::from(!flip[1]))),
            ),
            tint,
        );
    }
}

fn action(element: astrocore_core::ObjectId, id: u8, operation: &str) -> (&'static str, Vec<u8>) {
    (
        gameplay::wires::CMD_ACTION,
        gameplay::target_data(
            gameplay::VerbTarget::Object(element),
            Some(&format!("door_wire_{operation}_{id}")),
        ),
    )
}

pub(crate) fn draw(
    ctx: &egui::Context,
    icons: &mut Icons,
    view: Option<&gameplay::wires::View>,
) -> Commands {
    let mut out = Vec::new();
    let Some(view) = view else { return out };
    let mut open = true;
    egui::Window::new("Wires")
        .id(egui::Id::new("airlock_wires"))
        .open(&mut open)
        .resizable(false)
        .default_width(320.)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.strong(&view.name);
                ui.label(&view.serial);
            });
            ui.separator();
            let columns = view.lights.len().div_ceil(2).max(1);
            egui::Grid::new("wire_status_lights")
                .num_columns(columns)
                .show(ui, |ui| {
                    let ordered = (0..2).flat_map(|row| {
                        (0..columns).filter_map(move |column| view.lights.get(column * 2 + row))
                    });
                    for (i, light) in ordered.enumerate() {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.;
                            let label = match light.text.as_str() {
                                "Power" => "POWR",
                                "Bolts" => "BOLT",
                                "Bolt lights" => "BLIT",
                                "Timing" => "TIMR",
                                "Safety" => "SAFE",
                                "Access logging" => "LOG",
                                "AI control" => "AIA",
                                other => other,
                            };
                            ui.label(
                                egui::RichText::new(label)
                                    .monospace()
                                    .size(12.)
                                    .color(Color32::from_rgb(161, 166, 174)),
                            )
                            .on_hover_text(&light.text);
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(20.), egui::Sense::hover());
                            let color = wire_color(&light.color);
                            texture(
                                ui,
                                icons,
                                ctx,
                                "light_off_base",
                                rect,
                                color.gamma_multiply(0.5),
                            );
                            let time = ctx.input(|input| input.time);
                            let brightness = match light.state {
                                gameplay::wires::LightState::Off => 0.,
                                gameplay::wires::LightState::On => 1.,
                                gameplay::wires::LightState::BlinkingFast => {
                                    ((time % 0.2 - 0.1).abs() * 10.) as f32
                                }
                                gameplay::wires::LightState::BlinkingSlow => {
                                    let phase = time % 0.8;
                                    if phase < 0.3 {
                                        1.
                                    } else if phase < 0.4 {
                                        ((0.4 - phase) * 10.) as f32
                                    } else if phase < 0.7 {
                                        0.
                                    } else {
                                        ((phase - 0.7) * 10.) as f32
                                    }
                                }
                            };
                            if brightness > 0. {
                                texture(
                                    ui,
                                    icons,
                                    ctx,
                                    "light_on_base",
                                    rect,
                                    color.gamma_multiply(0.4 * brightness),
                                );
                            }
                        });
                        if i % columns == columns - 1 {
                            ui.end_row();
                        }
                    }
                });
            ui.separator();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.;
                ui.add_space(20.);
                for wire in &view.wires {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(20., 102.), egui::Sense::hover());
                    let top = Rect::from_min_size(rect.min, egui::vec2(20., 16.));
                    let bottom = top.translate(egui::vec2(0., 60.));
                    let middle =
                        Rect::from_min_size(rect.min + egui::vec2(2., 16.), egui::vec2(16., 50.));
                    texture(
                        ui,
                        icons,
                        ctx,
                        "contact",
                        top,
                        Color32::from_rgb(225, 202, 118),
                    );
                    texture(
                        ui,
                        icons,
                        ctx,
                        "contact",
                        bottom,
                        Color32::from_rgb(225, 202, 118),
                    );
                    let shape = 1 + wire.variant;
                    let state = if wire.cut { "_cut" } else { "" };
                    if wire.cut {
                        wire_texture(
                            ui,
                            icons,
                            ctx,
                            &format!("wire_{shape}_copper"),
                            middle,
                            Color32::from_rgb(255, 165, 0),
                            [wire.mirror, wire.flip],
                        );
                    }
                    wire_texture(
                        ui,
                        icons,
                        ctx,
                        &format!("wire_{shape}{state}"),
                        middle,
                        wire_color(&wire.color),
                        [wire.mirror, wire.flip],
                    );
                    ui.painter().text(
                        rect.center_bottom(),
                        egui::Align2::CENTER_BOTTOM,
                        wire.letter,
                        egui::FontId::proportional(12.),
                        Color32::GRAY,
                    );
                    let operation = if wire.cut { "Mend" } else { "Cut" };
                    let response = ui
                        .interact(
                            Rect::from_min_size(
                                rect.min + egui::vec2(0., 20.),
                                egui::vec2(20., 40.),
                            ),
                            ui.id().with((wire.id, "wire")),
                            egui::Sense::click(),
                        )
                        .on_hover_text(format!("{} · {operation}", wire.color));
                    if response.clicked() {
                        out.push(action(view.element, wire.id, operation));
                    }
                    for (index, contact) in [
                        Rect::from_min_size(rect.min, egui::vec2(20., 20.)),
                        Rect::from_min_size(rect.min + egui::vec2(0., 60.), egui::vec2(20., 20.)),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let response = ui
                            .interact(
                                contact,
                                ui.id().with((wire.id, index)),
                                egui::Sense::click(),
                            )
                            .on_hover_text("Pulse");
                        if response.clicked() {
                            out.push(action(view.element, wire.id, "Pulse"));
                        }
                    }
                }
            });
        });
    if !open {
        out.push((gameplay::wires::CMD_CLOSE, Vec::new()));
    }
    out
}
