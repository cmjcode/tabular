//! Registry global driver engine yang terpasang.
//!
//! Diisi saat startup dari folder plugin (`<data_dir>/plugins/drivers`) dan
//! bisa diubah saat runtime oleh Plugin Manager (install/uninstall/enable).

use super::{DriverError, DriverResult, EngineDescriptor, EngineDriver};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Asal driver, ditampilkan di Plugin Manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverSource {
    Wasm {
        path: std::path::PathBuf,
    },
    Sidecar {
        path: std::path::PathBuf,
    },
    /// Driver yang didaftarkan langsung dari kode (test, adapter internal).
    InProcess,
}

#[derive(Clone)]
pub struct RegisteredDriver {
    pub driver: Arc<dyn EngineDriver>,
    pub source: DriverSource,
    pub version: String,
}

#[derive(Default)]
pub struct DriverRegistry {
    drivers: BTreeMap<String, RegisteredDriver>,
}

impl DriverRegistry {
    pub fn register(&mut self, entry: RegisteredDriver) -> DriverResult<()> {
        let id = entry.driver.descriptor().id.clone();
        if !EngineDescriptor::is_valid_id(&id) {
            return Err(DriverError::Protocol(format!("invalid engine id '{id}'")));
        }
        if crate::models::enums::DatabaseType::is_builtin_id(&id) {
            return Err(DriverError::Protocol(format!(
                "engine id '{id}' is reserved for a builtin driver"
            )));
        }
        if let Some(prev) = self.drivers.insert(id.clone(), entry) {
            log::info!(
                "[DRIVER-PLUGIN] Replaced driver '{}' (was version {})",
                id,
                prev.version
            );
        }
        Ok(())
    }

    pub fn unregister(&mut self, id: &str) -> Option<RegisteredDriver> {
        self.drivers.remove(id)
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn EngineDriver>> {
        self.drivers.get(id).map(|e| e.driver.clone())
    }

    pub fn entry(&self, id: &str) -> Option<RegisteredDriver> {
        self.drivers.get(id).cloned()
    }

    pub fn descriptors(&self) -> Vec<EngineDescriptor> {
        self.drivers
            .values()
            .map(|e| e.driver.descriptor().clone())
            .collect()
    }

    pub fn entries(&self) -> Vec<RegisteredDriver> {
        self.drivers.values().cloned().collect()
    }
}

fn global() -> &'static RwLock<DriverRegistry> {
    static REGISTRY: OnceLock<RwLock<DriverRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(DriverRegistry::default()))
}

pub fn register(entry: RegisteredDriver) -> DriverResult<()> {
    global()
        .write()
        .map_err(|_| DriverError::Plugin("driver registry poisoned".into()))?
        .register(entry)
}

pub fn unregister(id: &str) -> Option<RegisteredDriver> {
    global().write().ok()?.unregister(id)
}

/// Driver untuk engine `id`, atau [`DriverError::NotInstalled`].
pub fn driver(id: &str) -> DriverResult<Arc<dyn EngineDriver>> {
    global()
        .read()
        .ok()
        .and_then(|r| r.get(id))
        .ok_or_else(|| DriverError::NotInstalled(id.to_string()))
}

pub fn descriptor(id: &str) -> Option<EngineDescriptor> {
    driver(id).ok().map(|d| d.descriptor().clone())
}

pub fn descriptors() -> Vec<EngineDescriptor> {
    global().read().map(|r| r.descriptors()).unwrap_or_default()
}

pub fn entries() -> Vec<RegisteredDriver> {
    global().read().map(|r| r.entries()).unwrap_or_default()
}

pub fn is_installed(id: &str) -> bool {
    global()
        .read()
        .map(|r| r.get(id).is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver_api::{ConnectParams, EngineSession};

    struct Dummy(EngineDescriptor);
    impl EngineDriver for Dummy {
        fn descriptor(&self) -> &EngineDescriptor {
            &self.0
        }
        fn connect(&self, _: ConnectParams) -> DriverResult<Arc<dyn EngineSession>> {
            Err(DriverError::Connect("dummy".into()))
        }
    }

    fn dummy(id: &str) -> RegisteredDriver {
        RegisteredDriver {
            driver: Arc::new(Dummy(EngineDescriptor {
                id: id.into(),
                name: id.into(),
                icon: None,
                default_port: None,
                standard_fields: Default::default(),
                options: vec![],
                capabilities: Default::default(),
            })),
            source: DriverSource::InProcess,
            version: "0.0.1".into(),
        }
    }

    #[test]
    fn rejects_invalid_and_builtin_ids() {
        let mut reg = DriverRegistry::default();
        assert!(reg.register(dummy("Bad Id")).is_err());
        assert!(reg.register(dummy("postgresql")).is_err());
        assert!(reg.register(dummy("sqlite")).is_err());
        assert!(reg.register(dummy("clickhouse")).is_ok());
        assert!(reg.get("clickhouse").is_some());
        assert_eq!(reg.descriptors().len(), 1);
        assert!(reg.unregister("clickhouse").is_some());
        assert!(reg.get("clickhouse").is_none());
    }
}
