//! Действия с таймером (`DoAfter` в SS14): надевание и снятие с задержкой, вскрытие двери
//! монтировкой. Пока идёт отсчёт, персонаж не должен сходить с места (`BreakOnMove`) и обязан
//! сохранять условия действия; иначе оно прерывается без последствий. Одновременно у персонажа
//! одно такое действие. Начало и конец рассылаются всем (`build.doafter`), чтобы клиент рисовал
//! полоску.

use astrocore_core::ObjectId;
use astrocore_math::glam::Vec3;
use serde::{Deserialize, Serialize};

/// Состояние действия с таймером у персонажа; пустое значение — действия нет.
pub const DOAFTER_COMPONENT: &str = "build.doafter";
pub const DOAFTER_MAX_BYTES: usize = 256;

/// На сколько можно сместиться, не прерывая действие, м (порог `BreakOnMove` в SS14).
pub const BREAK_DISTANCE_M: f32 = 0.3;
/// Время вскрытия двери монтировкой, с (`DoorComponent.PryTime` в SS14).
pub const PRY_TIME_S: f32 = 1.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum StructureWork {
    CutCable(ObjectId, f32),
    PipeAnchor(ObjectId, bool, f32),
    PipeDismantle(ObjectId, usize, f32),
    Repair(ObjectId, f32, f32),
    StripWall(ObjectId),
    StripReinforced(ObjectId, usize),
    StripGirder(ObjectId),
    ToggleAnchor(ObjectId, bool),
    Dismantle(ObjectId),
}

/// Что делает действие по завершении.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Action {
    Repair(RepairJob),
    Inflate {
        floor: astrocore_world::FloorId,
        cell: astrocore_math::GridCell,
        stack: ObjectId,
        proto: String,
        delay: f32,
    },
    Deflate {
        target: ObjectId,
        delay: f32,
    },
    Rcd(RcdJob),
    Barrier(BarrierJob),
    Frame(FrameJob),
    BuildPipe {
        recipe: usize,
        rotation: u8,
        floor: astrocore_world::FloorId,
        cell: astrocore_math::GridCell,
        stack: ObjectId,
    },
    RemoveTile {
        floor: astrocore_world::FloorId,
        cell: astrocore_math::GridCell,
        slab: ObjectId,
        hand: usize,
        tool: ObjectId,
    },
    WorkStructure {
        cell: (i32, i32),
        hand: usize,
        tool: ObjectId,
        operation: StructureWork,
    },
    Construct {
        reinforced: bool,
        cell: (i32, i32),
        hand: usize,
        stack: ObjectId,
        girder: Option<ObjectId>,
    },
    Equip {
        hand: usize,
        slot: String,
    },
    Unequip {
        slot: String,
        hand: Option<usize>,
    },
    Pry {
        element: ObjectId,
        hand: usize,
        tool: ObjectId,
    },
    /// Лечение предметом из руки: цель — персонаж (в том числе сам).
    Heal {
        hand: usize,
        target: astrocore_core::NetworkEntityId,
    },
    /// Инъекция шприцем или гипоспреем из руки в тело персонажа (в том числе своё).
    Inject {
        hand: usize,
        target: astrocore_core::NetworkEntityId,
    },
    /// Сканирование анализатором здоровья из руки.
    Scan {
        hand: usize,
        target: astrocore_core::NetworkEntityId,
    },
    /// Разряд дефибриллятора из руки в живого или в тело погибшего (цель — его владелец).
    Defib {
        hand: usize,
        target: astrocore_core::NetworkEntityId,
    },
    /// Смешивание предметом-смесителем: `target` — раствор в мире, `None` — раствор самого предмета.
    Mix {
        hand: usize,
        target: Option<ObjectId>,
        shake: bool,
    },
    /// Действие игровой системы (`systems/`): ход и прерывание — общие, проверку и итог делает
    /// сама система (`GameSystem::finish_job`).
    System(SystemJob),
    /// Обыск: снять предмет с цели (`take`) или надеть на неё активный из руки.
    Strip {
        target: astrocore_core::NetworkEntityId,
        /// Рука цели или слот одежды.
        hand: Option<usize>,
        slot: String,
        take: bool,
    },
    /// Пристегнуться к сиденью (`BuckleDoAfterEvent`).
    Buckle {
        strap: ObjectId,
    },
    /// Съесть или выпить укус из предмета в руке.
    Eat {
        hand: usize,
        item: ObjectId,
        /// Кого кормят; `None` — себя.
        target: Option<astrocore_core::NetworkEntityId>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RepairJob {
    pub target: ObjectId,
    pub prototype: astrocore_core::PrototypeId,
    pub hand: usize,
    pub tool: ObjectId,
    pub delay: f32,
    pub fuel: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FrameJob {
    pub rotation: u8,
    pub floor: astrocore_world::FloorId,
    pub cell: astrocore_math::GridCell,
    pub target: Option<ObjectId>,
    pub item: ObjectId,
    pub graph: String,
    pub node: String,
    pub next: String,
    pub delay: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BarrierJob {
    pub girder: Option<ObjectId>,
    pub floor: astrocore_world::FloorId,
    pub cell: astrocore_math::GridCell,
    pub proto: String,
    pub dismantle: bool,
    pub step: usize,
    pub item: ObjectId,
    pub delay: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RcdJob {
    pub device: ObjectId,
    pub floor: astrocore_world::FloorId,
    pub cell: astrocore_math::GridCell,
    pub rotation: u8,
    pub mode: String,
    pub target: String,
    pub object: Option<ObjectId>,
    pub cost: u32,
    pub delay: f32,
}
impl Action {
    pub(crate) fn construction_breaks_on_damage(&self) -> bool {
        matches!(
            self,
            Self::BuildPipe { .. }
                | Self::Repair(_)
                | Self::Pry { .. }
                | Self::Inflate { .. }
                | Self::Barrier(_)
                | Self::Rcd(_)
                | Self::WorkStructure { .. }
                | Self::Construct { girder: None, .. }
        )
    }
    /// Прерывается ли действие шагом в сторону: есть и пить можно на ходу (`BreakOnMove` у
    /// `EatingDoAfter` включён только при кормлении другого).
    pub(crate) fn breaks_on_move(&self) -> bool {
        // Дефибриллятор: `AllowDoAfterMovement` включён, шаг не прерывает разряд.
        match self {
            Self::System(job) => job.breaks_on_move,
            _ => !matches!(self, Self::Eat { target: None, .. } | Self::Defib { .. }),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Repair(_) => "repair",
            Self::Frame(_) | Self::Barrier(_) | Self::Rcd(_) | Self::Inflate { .. } => "construct",
            Self::Deflate { .. } => "disassemble",
            Self::BuildPipe { .. }
            | Self::RemoveTile { .. }
            | Self::WorkStructure { .. }
            | Self::Construct { .. } => "construct",
            Self::Equip { .. } => "equip",
            Self::Unequip { .. } => "unequip",
            Self::Pry { .. } => "pry",
            Self::Heal { .. } => "heal",
            Self::Inject { .. } => "inject",
            Self::Scan { .. } => "scan",
            Self::Defib { .. } => "defib",
            Self::System(job) => job.kind,
            Self::Mix { shake: true, .. } => "shake",
            Self::Mix { .. } => "mix",
            Self::Eat { .. } => "eat",
            Self::Buckle { .. } => "buckle",
            Self::Strip { .. } => "strip",
        }
    }
}

/// Действие системы с задержкой: какая система его завершит, что показать клиенту и над чем
/// оно идёт.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemJob {
    /// `GameSystem::id` системы, которая завершит действие.
    pub system: &'static str,
    /// Название для полосы действия у клиента (`DoAfterView::kind`).
    pub kind: &'static str,
    pub delay: f32,
    pub breaks_on_move: bool,
    /// Предмет или устройство, над которым идёт действие.
    pub object: Option<ObjectId>,
    /// Персонаж-цель.
    pub target: Option<astrocore_core::NetworkEntityId>,
    pub hand: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct DoAfter {
    pub(crate) last_damage: f32,
    pub(crate) action: Action,
    pub(crate) delay: f32,
    pub(crate) elapsed: f32,
    pub(crate) start: Vec3,
}

/// Для клиента: что за действие и сколько оно длится.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoAfterView {
    pub kind: String,
    pub delay_s: f32,
}

pub fn encode_doafter(v: Option<&DoAfterView>) -> Vec<u8> {
    v.map(|v| serde_json::to_vec(v).unwrap_or_default())
        .unwrap_or_default()
}

/// Разобрать состояние; `None` и для пустого значения, и для мусора.
pub fn decode_doafter(bytes: &[u8]) -> Option<DoAfterView> {
    if bytes.is_empty() || bytes.len() > DOAFTER_MAX_BYTES {
        return None;
    }
    serde_json::from_slice(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_round_trips_and_empty_means_no_action() {
        let v = DoAfterView {
            kind: "pry".into(),
            delay_s: 1.5,
        };
        assert_eq!(decode_doafter(&encode_doafter(Some(&v))), Some(v));
        assert_eq!(decode_doafter(&encode_doafter(None)), None);
        assert_eq!(decode_doafter(b"\xff garbage"), None);
    }
}
