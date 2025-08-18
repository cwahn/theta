use std::sync::OnceLock;

use directories::ProjectDirs;

use crate::persistence::persistent_actor::PersistentStorage;

static PROJECT_DIRS: OnceLock<ProjectDirs> = OnceLock::new();

pub struct ProjectDir;

// Implementation

impl ProjectDir {
    pub fn init(qualifier: &str, organization: &str, application: &str) {
        PROJECT_DIRS
            .set(
                ProjectDirs::from(qualifier, organization, application)
                    .expect("Failed to get project directories on this OS"),
            )
            .expect("ProjectDir should only be initialized once");
    }

    pub fn inst() -> &'static ProjectDirs {
        PROJECT_DIRS
            .get()
            .expect("ProjectDir should be initialized before accessing it")
    }

    fn path(&self, id: crate::actor::ActorId) -> std::path::PathBuf {
        let base = ProjectDir::inst().data_dir();
        base.join(format!("{}", id))
    }
}

impl PersistentStorage for ProjectDir {
    async fn try_read(&self, id: crate::actor::ActorId) -> Result<Vec<u8>, std::io::Error> {
        tokio::fs::read(self.path(id)).await
    }

    async fn try_write(
        &self,
        id: crate::actor::ActorId,
        bytes: Vec<u8>,
    ) -> Result<(), std::io::Error> {
        tokio::fs::write(self.path(id), bytes).await
    }
}
