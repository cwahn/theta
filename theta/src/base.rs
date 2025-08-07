#[cfg(feature = "std")]
use std::sync::LazyLock;

#[cfg(all(feature = "std", feature = "persistence"))]
use directories::ProjectDirs;
use uuid::Uuid;

pub type ImplId = Uuid;

// todo Open control to user
#[cfg(feature = "std")]
pub(crate) static PROJECT_DIRS: LazyLock<ProjectDirs> = LazyLock::new(|| {
    ProjectDirs::from("com", "example", "theta")
        .expect("Failed to get project directories on this OS")
});

// pub(crate) fn panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
//     if let Some(s) = payload.downcast_ref::<&str>() {
//         s.to_string()
//     } else if let Ok(s) = payload.downcast::<String>() {
//         *s
//     } else {
//         "Unknown panic payload".to_string()
//     }
// }
