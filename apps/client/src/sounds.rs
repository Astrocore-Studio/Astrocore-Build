//! Звуки SS14: шаги, двери, шорох предметов. Файлы читаются из `Audio` рядом с манифестом
//! рендера; без них клиент молчит.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use astrocore_client::Net;
use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use gameplay::{Hands, InventoryView};

/// Слышимость падает до нуля на этом расстоянии, м.
const HEARING_M: f32 = 14.0;

#[derive(Resource, Default)]
pub struct Sounds {
    steps: Vec<Handle<AudioSource>>,
    rustle: Vec<Handle<AudioSource>>,
    door_open: Option<Handle<AudioSource>>,
    door_close: Option<Handle<AudioSource>>,
    counter: usize,
    root: std::path::PathBuf,
    cache: HashMap<String, Option<Handle<AudioSource>>>,
}

/// Громкость по расстоянию до слушателя.
pub fn attenuation(distance: f32) -> f32 {
    (1.0 - distance / HEARING_M).clamp(0.0, 1.0)
}

impl Sounds {
    pub fn load(root: &Path, assets: &mut Assets<AudioSource>) -> Self {
        let mut load = |rel: &str| {
            std::fs::read(root.join("Audio").join(rel)).ok().map(|b| {
                assets.add(AudioSource {
                    bytes: Arc::from(b),
                })
            })
        };
        Self {
            steps: (1..=5)
                .filter_map(|i| load(&format!("Effects/Footsteps/floor{i}.ogg")))
                .collect(),
            rustle: (1..=5)
                .filter_map(|i| load(&format!("Effects/rustle{i}.ogg")))
                .collect(),
            door_open: load("Machines/airlock_open.ogg"),
            door_close: load("Machines/airlock_close.ogg"),
            counter: 0,
            root: root.to_path_buf(),
            cache: HashMap::new(),
        }
    }

    /// Сыграть звук по пути внутри `Audio` на расстоянии `distance` от слушателя; без файла — тишина.
    pub fn play_path(
        &mut self,
        commands: &mut Commands,
        assets: &mut Assets<AudioSource>,
        path: &str,
        distance: f32,
    ) {
        let v = attenuation(distance) * 0.8;
        if v <= 0.0 {
            return;
        }
        let root = self.root.clone();
        let handle = self.cache.entry(path.to_string()).or_insert_with(|| {
            std::fs::read(root.join("Audio").join(path)).ok().map(|b| {
                assets.add(AudioSource {
                    bytes: Arc::from(b),
                })
            })
        });
        if let Some(h) = handle {
            play(commands, h, v);
        }
    }

    fn pick(list: &[Handle<AudioSource>], n: usize) -> Option<&Handle<AudioSource>> {
        (!list.is_empty()).then(|| &list[n % list.len()])
    }

    /// Шаг на расстоянии `distance` от слушателя.
    pub fn step(&mut self, commands: &mut Commands, distance: f32) {
        self.counter += 1;
        let v = attenuation(distance) * 0.5;
        if v > 0.0
            && let Some(h) = Self::pick(&self.steps, self.counter)
        {
            play(commands, h, v);
        }
    }

    pub fn rustle(&mut self, commands: &mut Commands) {
        self.counter += 1;
        if let Some(h) = Self::pick(&self.rustle, self.counter) {
            play(commands, h, 0.5);
        }
    }

    pub fn door(&self, commands: &mut Commands, open: bool) {
        let h = if open {
            &self.door_open
        } else {
            &self.door_close
        };
        if let Some(h) = h {
            play(commands, h, 0.7);
        }
    }
}

fn play(commands: &mut Commands, handle: &Handle<AudioSource>, volume: f32) {
    commands.spawn((
        AudioPlayer::new(handle.clone()),
        PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume)),
    ));
}

/// Двери: звук при смене состояния любой двери, о которой знает клиент.
pub fn door_sounds(
    net: Option<NonSend<Net>>,
    sounds: Option<Res<Sounds>>,
    mut commands: Commands,
    mut last: Local<HashMap<astrocore_core::NetworkEntityId, bool>>,
) {
    let (Some(net), Some(sounds)) = (net, sounds) else {
        return;
    };
    for id in net.core.door_ids() {
        let Some(open) = net.core.door_open(id) else {
            continue;
        };
        if let Some(prev) = last.insert(id, open)
            && prev != open
        {
            sounds.door(&mut commands, open);
        }
    }
}

/// Шорох, когда у игрока меняется содержимое рук или надетое.
pub fn item_sounds(
    loadout: Res<crate::Loadout>,
    sounds: Option<ResMut<Sounds>>,
    mut commands: Commands,
    mut last: Local<Option<(Hands, Vec<String>)>>,
) {
    let Some(mut sounds) = sounds else { return };
    let (Some(hands), Some(view)) = (loadout.hands.as_ref(), loadout.inventory.as_ref()) else {
        return;
    };
    let worn = worn_ids(view);
    let now = (hands.clone(), worn);
    if let Some(prev) = last.as_ref()
        && (prev
            .0
            .iter()
            .map(|(_, _, i)| i.map(|x| x.id))
            .collect::<Vec<_>>()
            != now
                .0
                .iter()
                .map(|(_, _, i)| i.map(|x| x.id))
                .collect::<Vec<_>>()
            || prev.1 != now.1)
    {
        sounds.rustle(&mut commands);
    }
    *last = Some(now);
}

fn worn_ids(view: &InventoryView) -> Vec<String> {
    view.inventory
        .iter()
        .filter_map(|(d, i)| Some(format!("{}:{}", d.name, i?.id.as_uuid())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::attenuation;

    #[test]
    fn sounds_fade_with_distance_and_vanish_when_far() {
        assert_eq!(attenuation(0.0), 1.0);
        assert!(attenuation(7.0) > 0.4 && attenuation(7.0) < 0.6);
        assert_eq!(attenuation(40.0), 0.0);
    }
}
