//! Desktop notifications over D-Bus (org.freedesktop.Notifications), with
//! action buttons. The shell's notification server (end4-pC's quickshell,
//! mako, swaync, dunst, …) draws them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::Value;

const DEST: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

#[derive(Clone, Debug, Default)]
pub struct Notification {
    pub summary: String,
    pub body: String,
    pub icon: String,
    /// Shown as the notification's image (the capture itself).
    pub image: Option<PathBuf>,
    /// (key, label). The key "default" is a click on the notification.
    pub actions: Vec<(String, String)>,
    pub timeout_ms: i32,
    pub urgency: u8,
}

/// What happened to a notification we waited on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Action(String),
    Closed,
    TimedOut,
}

pub struct Notifier {
    conn: Connection,
    signals: mpsc::Receiver<(String, u32, String)>,
}

impl Notifier {
    pub fn connect() -> Result<Self, String> {
        let conn = Connection::session().map_err(|e| format!("D-Bus: {e}"))?;
        // Subscribe before sending, so a quick click can't be missed.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(DEST)
            .map_err(|e| e.to_string())?
            .path(PATH)
            .map_err(|e| e.to_string())?
            .build();
        let iter = MessageIterator::for_match_rule(rule, &conn, Some(32)).map_err(|e| format!("D-Bus: {e}"))?;
        let (tx, signals) = mpsc::channel();
        std::thread::spawn(move || {
            for msg in iter.flatten() {
                let header = msg.header();
                let member = header.member().map(|m| m.to_string()).unwrap_or_default();
                let parsed = match member.as_str() {
                    "ActionInvoked" => msg.body().deserialize::<(u32, String)>().ok().map(|(id, key)| (member.clone(), id, key)),
                    "NotificationClosed" => msg.body().deserialize::<(u32, u32)>().ok().map(|(id, _)| (member.clone(), id, String::new())),
                    _ => None,
                };
                if let Some(p) = parsed {
                    if tx.send(p).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Self { conn, signals })
    }

    pub fn send(&self, n: &Notification) -> Result<u32, String> {
        let actions: Vec<&str> = n.actions.iter().flat_map(|(k, l)| [k.as_str(), l.as_str()]).collect();
        let mut hints: HashMap<&str, Value> = HashMap::new();
        hints.insert("urgency", Value::U8(n.urgency));
        hints.insert("desktop-entry", Value::from("material_3_screenshots"));
        let image = n.image.as_ref().map(|p| p.to_string_lossy().into_owned());
        if let Some(image) = &image {
            hints.insert("image-path", Value::from(image.as_str()));
        }
        let reply = self
            .conn
            .call_method(
                Some(DEST),
                PATH,
                Some(DEST),
                "Notify",
                &("Screenshots", 0u32, n.icon.as_str(), n.summary.as_str(), n.body.as_str(), actions, hints, n.timeout_ms),
            )
            .map_err(|e| format!("Notify: {e}"))?;
        reply.body().deserialize::<u32>().map_err(|e| e.to_string())
    }

    /// Wait for the user to press an action or dismiss notification `id`.
    pub fn wait(&self, id: u32, timeout: Duration) -> Outcome {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.signals.recv_timeout(left) {
                Ok((member, sid, key)) if sid == id => {
                    return if member == "ActionInvoked" { Outcome::Action(key) } else { Outcome::Closed };
                }
                Ok(_) => continue,
                Err(_) => return Outcome::TimedOut,
            }
        }
    }

    pub fn close(&self, id: u32) {
        let _ = self.conn.call_method(Some(DEST), PATH, Some(DEST), "CloseNotification", &(id,));
    }
}

/// Fire and forget.
pub fn send(n: &Notification) {
    if let Err(e) = Notifier::connect().and_then(|c| c.send(n)) {
        eprintln!("m3-screenshots: couldn't notify: {e}");
    }
}

pub fn error(summary: &str, body: &str) {
    send(&Notification {
        summary: summary.into(),
        body: body.into(),
        icon: "dialog-error".into(),
        timeout_ms: 8000,
        urgency: 2,
        ..Default::default()
    });
}
