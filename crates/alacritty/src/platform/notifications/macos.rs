use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use alacritty_terminal::protocols::notifications::Notification;
use futures_lite::future;
use futures_timer::Delay;
use mac_usernotifications::{Action, InterruptionLevel};
use objc2_app_kit::{NSImage, NSWorkspace};
use objc2_foundation::NSString;
use parking_lot::Mutex;
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;

use super::Feedback;
use crate::app::{Event, EventType};
use crate::workspace::layout::PaneId;

const MAX_WORKERS: usize = 64;
static WORKERS: AtomicUsize = AtomicUsize::new(0);

fn button_namespace(buttons: &[String]) -> Option<String> {
    static CATEGORIES: OnceLock<Mutex<HashSet<u64>>> = OnceLock::new();
    let mut hash = DefaultHasher::new();
    buttons.hash(&mut hash);
    let hash = hash.finish();
    let mut categories = CATEGORIES.get_or_init(Default::default).lock();
    if !buttons.is_empty() && !categories.contains(&hash) {
        // The native helper retains categories for the process lifetime.
        if categories.len() >= 64 {
            return None;
        }
        categories.insert(hash);
    }
    Some(format!("{hash:016x}"))
}

fn icon_file(notification: &Notification) -> Option<tempfile::NamedTempFile> {
    for name in notification.icons.iter().chain(std::iter::once(&notification.application)) {
        let symbol = match name.as_str() {
            "error" => Some("xmark.octagon.fill"),
            "warn" | "warning" => Some("exclamationmark.triangle.fill"),
            "info" => Some("info.circle.fill"),
            "question" | "help" => Some("questionmark.circle.fill"),
            "file-manager" => Some("folder.fill"),
            "system-monitor" => Some("waveform.path.ecg"),
            "text-editor" => Some("doc.text.fill"),
            _ => None,
        };
        let image = if let Some(symbol) = symbol {
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                None,
            )
        } else if !name.is_empty() {
            let workspace = NSWorkspace::sharedWorkspace();
            workspace
                .URLForApplicationWithBundleIdentifier(&NSString::from_str(name))
                .and_then(|url| url.path())
                .map(|path| workspace.iconForFile(&path))
        } else {
            None
        };
        if let Some(data) = image.and_then(|image| image.TIFFRepresentation()) {
            let file = tempfile::Builder::new()
                .prefix("alacritty-notification-")
                .suffix(".tiff")
                .tempfile()
                .ok()?;
            if data
                .writeToFile_atomically(&NSString::from_str(&file.path().to_string_lossy()), false)
            {
                return Some(file);
            }
        }
    }
    None
}

struct WorkerPermit;
impl WorkerPermit {
    fn acquire(limit: usize) -> Option<Self> {
        if WORKERS.fetch_add(1, Ordering::Relaxed) >= limit {
            WORKERS.fetch_sub(1, Ordering::Relaxed);
            None
        } else {
            Some(Self)
        }
    }
}
impl Drop for WorkerPermit {
    fn drop(&mut self) {
        WORKERS.fetch_sub(1, Ordering::Relaxed);
    }
}

fn native_request(
    notification: &Notification,
    native_id: &str,
    thread_id: &str,
    namespace: &str,
    icon: Option<&std::path::Path>,
) -> mac_usernotifications::Notification {
    let mut request = mac_usernotifications::Notification::new()
        .id(native_id)
        .title(&notification.title)
        .message(&notification.body)
        .thread_id(thread_id)
        .interruption_level(match notification.urgency {
            0 => InterruptionLevel::Passive,
            2 => InterruptionLevel::TimeSensitive,
            _ => InterruptionLevel::Active,
        });
    if notification.sound != "silent" {
        request = request.default_sound();
    }
    for (index, label) in notification.buttons.iter().enumerate() {
        request = request.action(Action::button(format!("{namespace}-{}", index + 1), label));
    }
    if let Some(icon) = icon {
        request = request.image_path(icon.to_string_lossy());
    }
    request
}

pub struct NativeNotifications {
    window: WindowId,
    pane: PaneId,
    proxy: EventLoopProxy<Event>,
    pending: HashMap<u64, Arc<AtomicBool>>,
}

impl NativeNotifications {
    pub fn new(window: WindowId, pane: PaneId, proxy: EventLoopProxy<Event>) -> Self {
        Self { window, pane, proxy, pending: HashMap::new() }
    }

    fn prefix(&self) -> String {
        format!("alacritty-{}-{}-{}-", std::process::id(), u64::from(self.window), self.pane)
    }

    fn event(&self, feedback: Feedback) -> Event {
        Event::new(EventType::NotificationFeedback(feedback), self.window).with_pane(self.pane)
    }

    pub fn show(&mut self, notification: &Notification) {
        let serial = notification.serial;
        let Some(namespace) = button_namespace(&notification.buttons) else {
            let _ = self.proxy.send_event(self.event(Feedback::Failed(serial)));
            return;
        };
        let Some(permit) = WorkerPermit::acquire(MAX_WORKERS - 4) else {
            let _ = self.proxy.send_event(self.event(Feedback::Failed(serial)));
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        self.pending.insert(serial, cancelled.clone());
        let native_id = format!("{}{serial}", self.prefix());
        let icon = icon_file(notification);
        let notification = notification.clone();
        let (window, pane, proxy) = (self.window, self.pane, self.proxy.clone());
        let failed = self.event(Feedback::Failed(serial));
        let fallback_proxy = self.proxy.clone();
        let result = std::thread::Builder::new().name("notification".into()).spawn(move || {
            let _permit = permit;
            let send = |feedback| {
                let _ = proxy.send_event(
                    Event::new(EventType::NotificationFeedback(feedback), window).with_pane(pane),
                );
            };
            let outcome = future::block_on(async {
                mac_usernotifications::check_bundle()?;
                if !mac_usernotifications::request_auth().await?
                    || cancelled.load(Ordering::Relaxed)
                {
                    return Ok(None);
                }
                let request = native_request(
                    &notification,
                    &native_id,
                    &format!("alacritty-{}-{pane}", u64::from(window)),
                    &namespace,
                    icon.as_ref().map(|file| file.path()),
                );
                let handle = request.send().await?;
                let response = future::or(async { handle.response().await.map(Some) }, async {
                    loop {
                        if cancelled.load(Ordering::Relaxed) {
                            return Ok(None);
                        }
                        Delay::new(Duration::from_millis(500)).await;
                        if !mac_usernotifications::get_delivered_notification_ids()
                            .await
                            .contains(&native_id)
                        {
                            return Ok(None);
                        }
                    }
                })
                .await;
                if cancelled.load(Ordering::Relaxed) {
                    mac_usernotifications::cancel_pending(&native_id).await;
                    mac_usernotifications::close_delivered(&native_id).await;
                    return Ok(None);
                }
                response
            });
            match outcome {
                Ok(Some(response)) if response.is_default_action() => {
                    send(Feedback::Activated { serial, button: None })
                },
                Ok(Some(response)) if !response.is_dismiss_action() && !response.is_timed_out() => {
                    if let Some(button) = response
                        .action_identifier
                        .strip_prefix(&format!("{namespace}-"))
                        .and_then(|button| button.parse().ok())
                    {
                        send(Feedback::Activated { serial, button: Some(button) });
                    } else {
                        send(Feedback::Closed(serial));
                    }
                },
                Ok(_) => send(Feedback::Closed(serial)),
                Err(err) => {
                    log::warn!("Unable to deliver desktop notification: {err}");
                    send(Feedback::Failed(serial));
                },
            }
        });
        if let Err(err) = result {
            log::warn!("Unable to start notification worker: {err}");
            let _ = fallback_proxy.send_event(failed);
        }
    }

    pub fn close(&mut self, serial: u64) {
        if let Some(cancelled) = self.pending.remove(&serial) {
            cancelled.store(true, Ordering::Relaxed);
            if mac_usernotifications::check_bundle().is_ok() {
                let id = format!("{}{serial}", self.prefix());
                mac_usernotifications::blocking::cancel_pending(&id);
                mac_usernotifications::blocking::close_delivered(&id);
            }
        }
    }

    pub fn query_alive(&self, query: String) {
        let Some(permit) = WorkerPermit::acquire(MAX_WORKERS) else { return };
        let (prefix, window, pane, proxy) =
            (self.prefix(), self.window, self.pane, self.proxy.clone());
        let _ = std::thread::Builder::new().name("notification-query".into()).spawn(move || {
            let _permit = permit;
            let serials = if mac_usernotifications::check_bundle().is_ok() {
                future::block_on(mac_usernotifications::get_delivered_notification_ids())
                    .iter()
                    .filter_map(|id| id.strip_prefix(&prefix)?.parse().ok())
                    .collect()
            } else {
                vec![]
            };
            let _ = proxy.send_event(
                Event::new(
                    EventType::NotificationFeedback(Feedback::Alive { query, serials }),
                    window,
                )
                .with_pane(pane),
            );
        });
    }
}

impl Drop for NativeNotifications {
    fn drop(&mut self) {
        for serial in self.pending.keys().copied().collect::<Vec<_>>() {
            self.close(serial);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires running this test binary from a signed macOS app bundle"]
    fn native_permission_status() {
        mac_usernotifications::check_bundle().unwrap();
        let settings = mac_usernotifications::blocking::get_notification_settings().unwrap();
        println!("NATIVE_NOTIFICATION_AUTH={:?}", settings.authorization_status);
    }

    #[test]
    #[ignore = "requires a signed macOS app bundle and notification permission"]
    fn native_delivery_round_trip() {
        mac_usernotifications::check_bundle().unwrap();
        if std::env::var_os("ALACRITTY_TEST_REQUEST_NOTIFICATIONS").is_some() {
            assert!(
                mac_usernotifications::blocking::request_auth().unwrap(),
                "macOS denied notification permission"
            );
        }
        let settings = mac_usernotifications::blocking::get_notification_settings().unwrap();
        assert!(
            matches!(
                settings.authorization_status,
                mac_usernotifications::AuthorizationStatus::Authorized
                    | mac_usernotifications::AuthorizationStatus::Provisional
            ),
            "Notification permission is required: {:?}",
            settings.authorization_status
        );
        let notification = Notification {
            title: "Alacritty protocol check".into(),
            body: "Checking native OSC 99 delivery and cleanup.".into(),
            sound: "silent".into(),
            ..Default::default()
        };
        let id = format!("alacritty-protocol-test-{}", std::process::id());
        let handle = native_request(&notification, &id, "protocol-tests", "test", None)
            .send_blocking()
            .unwrap();
        let delivered = future::block_on(async {
            for _ in 0..30 {
                if mac_usernotifications::get_delivered_notification_ids().await.contains(&id) {
                    return true;
                }
                Delay::new(Duration::from_millis(100)).await;
            }
            false
        });
        future::block_on(mac_usernotifications::close_delivered(&id));
        assert!(delivered, "macOS did not list the delivered notification");
        assert!(
            !future::block_on(mac_usernotifications::get_delivered_notification_ids())
                .contains(&id)
        );
        drop(handle);
    }
}
