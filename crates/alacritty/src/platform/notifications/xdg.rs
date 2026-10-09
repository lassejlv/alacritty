use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use alacritty_terminal::protocols::notifications::Notification;
use futures_lite::{StreamExt, future};
use futures_timer::Delay;
use parking_lot::Mutex;
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;
use zbus::zvariant::Value;

use super::Feedback;
use crate::app::{Event, EventType};
use crate::workspace::layout::PaneId;

static WORKERS: AtomicUsize = AtomicUsize::new(0);
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct NativeNotifications {
    window: WindowId,
    pane: PaneId,
    proxy: EventLoopProxy<Event>,
    cancelled: HashMap<u64, Arc<AtomicBool>>,
    live: Arc<Mutex<HashMap<u64, u32>>>,
}

impl NativeNotifications {
    pub fn new(window: WindowId, pane: PaneId, proxy: EventLoopProxy<Event>) -> Self {
        Self { window, pane, proxy, cancelled: HashMap::new(), live: Default::default() }
    }

    fn event(&self, feedback: Feedback) -> Event {
        Event::new(EventType::NotificationFeedback(feedback), self.window).with_pane(self.pane)
    }

    pub fn show(&mut self, notification: &Notification) {
        let serial = notification.serial;
        if WORKERS.fetch_add(1, Ordering::Relaxed) >= 64 {
            WORKERS.fetch_sub(1, Ordering::Relaxed);
            let _ = self.proxy.send_event(self.event(Feedback::Failed(serial)));
            return;
        }
        let permit = Permit;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancelled.insert(serial, cancelled.clone());
        let (window, pane, proxy, live) =
            (self.window, self.pane, self.proxy.clone(), self.live.clone());
        let notification = notification.clone();
        let failed = self.event(Feedback::Failed(serial));
        let fallback_proxy = self.proxy.clone();
        let result = std::thread::Builder::new().name("notification".into()).spawn(move || {
            let _permit = permit;
            let result: Result<Feedback, zbus::Error> = future::block_on(async {
                let connection = zbus::Connection::session().await?;
                let service = zbus::Proxy::new(
                    &connection,
                    "org.freedesktop.Notifications",
                    "/org/freedesktop/Notifications",
                    "org.freedesktop.Notifications",
                )
                .await?;
                let mut signals = service.receive_all_signals().await?;
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(Feedback::Closed(serial));
                }
                let mut actions = vec!["default".to_string(), "Open".to_string()];
                for (index, label) in notification.buttons.iter().enumerate() {
                    actions.extend([(index + 1).to_string(), label.clone()]);
                }
                let mut hints = HashMap::new();
                hints.insert("urgency", Value::U8(notification.urgency));
                hints.insert("suppress-sound", Value::Bool(notification.sound == "silent"));
                hints.insert("desktop-entry", Value::from("Alacritty"));
                let icon =
                    match notification.icons.first().map(String::as_str).unwrap_or("Alacritty") {
                        "error" => "dialog-error",
                        "warn" | "warning" => "dialog-warning",
                        "info" => "dialog-information",
                        "question" | "help" => "dialog-question",
                        "file-manager" => "system-file-manager",
                        "system-monitor" => "utilities-system-monitor",
                        "text-editor" => "accessories-text-editor",
                        name => name,
                    };
                let body = notification
                    .body
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                let native: u32 = service
                    .call(
                        "Notify",
                        &(
                            "Alacritty",
                            0u32,
                            icon,
                            &notification.title,
                            body,
                            actions,
                            hints,
                            if notification.persistent { 0i32 } else { -1i32 },
                        ),
                    )
                    .await?;
                live.lock().insert(serial, native);
                let feedback = future::or(
                    async {
                        while let Some(signal) = signals.next().await {
                            let header = signal.header();
                            match header.member().map(|name| name.as_str()) {
                                Some("ActionInvoked") => {
                                    if let Ok((id, action)) =
                                        signal.body().deserialize::<(u32, String)>()
                                        && id == native
                                    {
                                        return Feedback::Activated {
                                            serial,
                                            button: action.parse().ok(),
                                        };
                                    }
                                },
                                Some("NotificationClosed") => {
                                    if let Ok((id, _)) = signal.body().deserialize::<(u32, u32)>()
                                        && id == native
                                    {
                                        return Feedback::Closed(serial);
                                    }
                                },
                                _ => (),
                            }
                        }
                        Feedback::Closed(serial)
                    },
                    async {
                        while !cancelled.load(Ordering::Relaxed) {
                            Delay::new(Duration::from_millis(100)).await;
                        }
                        Feedback::Closed(serial)
                    },
                )
                .await;
                let _: Result<(), _> = service.call("CloseNotification", &(native,)).await;
                Ok(feedback)
            });
            live.lock().remove(&serial);
            let feedback = result.unwrap_or_else(|error| {
                log::warn!("Unable to deliver desktop notification: {error}");
                Feedback::Failed(serial)
            });
            let _ = proxy.send_event(
                Event::new(EventType::NotificationFeedback(feedback), window).with_pane(pane),
            );
        });
        if let Err(error) = result {
            log::warn!("Unable to start notification worker: {error}");
            let _ = fallback_proxy.send_event(failed);
        }
    }

    pub fn close(&mut self, serial: u64) {
        if let Some(cancelled) = self.cancelled.remove(&serial) {
            cancelled.store(true, Ordering::Relaxed);
        }
        self.live.lock().remove(&serial);
    }

    pub fn query_alive(&self, query: String) {
        let serials = self.live.lock().keys().copied().collect();
        let _ = self.proxy.send_event(self.event(Feedback::Alive { query, serials }));
    }
}

impl Drop for NativeNotifications {
    fn drop(&mut self) {
        for cancelled in self.cancelled.values() {
            cancelled.store(true, Ordering::Relaxed);
        }
    }
}
