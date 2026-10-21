//! Notifications as Screenshots, through material-3-ui's `notify`.

use material_3_ui::notify::{self as shared, Notification, Urgency};

pub use material_3_ui::notify::{Notifier, Outcome};

/// A Screenshots notification: normal urgency, shown for 8 seconds.
pub fn notification(summary: &str, body: impl Into<String>, icon: &str) -> Notification {
    Notification {
        body: body.into(),
        icon: icon.into(),
        timeout_ms: 8000,
        urgency: Urgency::Normal,
        desktop_entry: Some("material_3_screenshots".into()),
        ..Notification::new("Screenshots", summary)
    }
}

/// Fire and forget.
pub fn send(n: &Notification) {
    if let Err(e) = shared::send(n) {
        eprintln!("m3-screenshots: couldn't notify: {e}");
    }
}

pub fn error(summary: &str, body: &str) {
    send(&Notification { icon: "dialog-error".into(), urgency: Urgency::Critical, ..notification(summary, body, "") });
}
