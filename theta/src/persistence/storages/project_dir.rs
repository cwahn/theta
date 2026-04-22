use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crate::persistence::persistent_actor::PersistentStorage;

/// Cached project data directory path for local filesystem storage.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Local filesystem storage backend for actor persistence.
pub struct LocalFs;

impl LocalFs {
    /// Initialize the local filesystem storage with the given path.
    ///
    /// # Panics
    ///
    /// Panics if called more than once (storage path is already initialized).
    pub fn init(path: &Path) {
        DATA_DIR
            .set(path.to_path_buf())
            .expect("Failed to set project directory");
    }

    /// Get the initialized storage path.
    ///
    /// # Panics
    ///
    /// Panics if [`LocalFs::init`] has not been called.
    pub fn inst() -> &'static PathBuf {
        DATA_DIR
            .get()
            .expect("ProjectDir should be initialized before accessing it")
    }

    fn path(id: crate::actor::ActorId) -> std::path::PathBuf {
        Self::inst().join(format!("{id}"))
    }
}

impl PersistentStorage for LocalFs {
    async fn try_read(&self, id: crate::actor::ActorId) -> Result<Vec<u8>, anyhow::Error> {
        Ok(tokio::fs::read(Self::path(id)).await?)
    }

    async fn try_write(
        &self,
        id: crate::actor::ActorId,
        bytes: Vec<u8>,
    ) -> Result<(), anyhow::Error> {
        let path = Self::path(id);

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        Ok(tokio::fs::write(&path, bytes).await?)
    }
}
