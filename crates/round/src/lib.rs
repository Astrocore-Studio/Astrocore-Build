use std::path::Path;
use std::sync::Arc;

use astrocore_api::{ModuleHost, ReplicationRegistry};
use astrocore_network::server::{AllowAll, GameServer, ServerNetConfig};

#[derive(Debug, thiserror::Error)]
pub enum RoundError {
    #[error("полигон: {0}")]
    Polygon(#[from] polygon::PolygonError),
    #[error("модуль: {0}")]
    Module(String),
}

/// Настройки раунда.
#[derive(Debug, Clone)]
pub struct RoundConfig {
    pub tick_hz: u8,
    /// Разрешить игрокам править структуру мира (отладка; по умолчанию выключено).
    pub allow_edit: bool,
}

impl Default for RoundConfig {
    fn default() -> Self {
        Self {
            tick_hz: 20,
            allow_edit: false,
        }
    }
}

/// Один раунд: мир строится с нуля, состояние игровых модулей — чистое. Всё, что случилось в
/// раунде, остаётся в раунде: новый раунд — новый `GameServer`.
pub struct Round {
    pub number: u32,
    pub server: GameServer,
}

impl Round {
    /// Начать раунд с номером `number` из содержимого в `content`.
    pub fn start(number: u32, content: &Path, config: &RoundConfig) -> Result<Self, RoundError> {
        let registry = Arc::new(polygon::load_registry(content)?);
        let mut world = polygon::build_world()?;
        polygon::place_items(&mut world, &registry)?;

        let mut modules = ModuleHost::new();
        let module = gameplay::GameplayModule::new(registry)
            .map_err(|e| RoundError::Module(e.to_string()))?
            .with_spawn(astrocore_math::glam::Vec3::from_array(polygon::spawn()));
        modules
            .register(Box::new(module))
            .map_err(|e| RoundError::Module(e.to_string()))?;
        modules
            .finalize()
            .map_err(|e| RoundError::Module(e.to_string()))?;
        let mut replication = ReplicationRegistry::new();
        gameplay::register_replication(&mut replication)
            .map_err(|e| RoundError::Module(e.to_string()))?;

        let net = ServerNetConfig {
            tick_hz: config.tick_hz,
            spawn: astrocore_math::glam::Vec3::from_array(polygon::spawn()),
            ..ServerNetConfig::default()
        };
        let server = GameServer::new(
            world,
            net,
            Box::new(AllowAll {
                can_edit: config.allow_edit,
            }),
        )
        .with_modules(modules)
        .with_replication(replication);
        Ok(Self { number, server })
    }

    /// Следующий раунд с нуля: прежний сервер отбрасывается вместе со всем состоянием.
    pub fn restart(&mut self, content: &Path, config: &RoundConfig) -> Result<(), RoundError> {
        *self = Self::start(self.number + 1, content, config)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
