use uuid::Uuid;

pub(crate) type ImplId = Uuid;

pub(crate) fn panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Ok(s) = payload.downcast::<String>() {
        *s
    } else {
        "Unknown panic payload".to_string()
    }
}
