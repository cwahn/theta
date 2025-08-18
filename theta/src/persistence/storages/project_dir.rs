use crate::persistence::persistent_actor::PersistentStorage;
use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

pub struct LocalFs;

// Implementation

impl LocalFs {
    pub fn init(path: &Path) {
        DATA_DIR
            .set(path.to_path_buf())
            .expect("Failed to set project directory");
    }

    pub fn inst() -> &'static PathBuf {
        DATA_DIR
            .get()
            .expect("ProjectDir should be initialized before accessing it")
    }

    fn path(&self, id: crate::actor::ActorId) -> std::path::PathBuf {
        LocalFs::inst().join(format!("{}", id))
    }
}

impl PersistentStorage for LocalFs {
    async fn try_read(&self, id: crate::actor::ActorId) -> Result<Vec<u8>, anyhow::Error> {
        Ok(tokio::fs::read(self.path(id)).await?)
    }

    async fn try_write(
        &self,
        id: crate::actor::ActorId,
        bytes: Vec<u8>,
    ) -> Result<(), anyhow::Error> {
        Ok(tokio::fs::write(self.path(id), bytes).await?)
    }
}
