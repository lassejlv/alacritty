use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use alacritty_terminal::protocols::notifications::Notification;
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastNotification, ToastNotificationManager, ToastNotificationPriority,
};
use windows::core::{HSTRING, Interface};
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;

use super::Feedback;
use crate::app::{Event, EventType};
use crate::workspace::layout::PaneId;

const APP_ID: &str = "org.alacritty.Alacritty";

struct Toast {
    notification: ToastNotification,
    activated: i64,
    dismissed: i64,
    failed: i64,
}

pub struct NativeNotifications {
    window: WindowId,
    pane: PaneId,
    proxy: EventLoopProxy<Event>,
    group: HSTRING,
    active: HashMap<u64, Toast>,
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

impl NativeNotifications {
    pub fn new(window: WindowId, pane: PaneId, proxy: EventLoopProxy<Event>) -> Self {
        let mut hash = DefaultHasher::new();
        (std::process::id(), window, pane).hash(&mut hash);
        Self {
            window,
            pane,
            proxy,
            group: HSTRING::from(format!("{:016x}", hash.finish())),
            active: HashMap::new(),
        }
    }

    fn event(&self, feedback: Feedback) -> Event {
        Event::new(EventType::NotificationFeedback(feedback), self.window).with_pane(self.pane)
    }

    pub fn show(&mut self, notification: &Notification) {
        if let Err(error) = self.show_inner(notification) {
            log::warn!("Unable to deliver desktop notification: {error}");
            let _ = self.proxy.send_event(self.event(Feedback::Failed(notification.serial)));
        }
    }

    fn show_inner(
        &mut self,
        notification: &Notification,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let registration = windows_registry::CURRENT_USER
            .create(format!(r"Software\Classes\AppUserModelId\{APP_ID}"))?;
        registration.set_string("DisplayName", "Alacritty")?;
        let mut xml = format!(
            "<toast><visual><binding \
             template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual>",
            escape(&notification.title),
            escape(&notification.body)
        );
        if notification.sound == "silent" {
            xml.push_str("<audio silent=\"true\"/>");
        }
        if !notification.buttons.is_empty() {
            xml.push_str("<actions>");
            for (index, label) in notification.buttons.iter().take(5).enumerate() {
                xml.push_str(&format!(
                    "<action content=\"{}\" arguments=\"{}\" activationType=\"foreground\"/>",
                    escape(label),
                    index + 1
                ));
            }
            xml.push_str("</actions>");
        }
        xml.push_str("</toast>");
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        toast.SetTag(&HSTRING::from(format!("{:x}", notification.serial)))?;
        toast.SetGroup(&self.group)?;
        if notification.urgency == 2 {
            toast.SetPriority(ToastNotificationPriority::High)?;
        }
        let serial = notification.serial;
        let (window, pane, proxy) = (self.window, self.pane, self.proxy.clone());
        let activated = toast.Activated(&TypedEventHandler::new(
            move |_, args: windows::core::Ref<'_, windows::core::IInspectable>| {
                let button = args
                    .as_ref()
                    .and_then(|args| args.cast::<ToastActivatedEventArgs>().ok())
                    .and_then(|args| args.Arguments().ok())
                    .filter(|args| !args.is_empty())
                    .and_then(|args| args.to_string().parse().ok());
                let _ = proxy.send_event(
                    Event::new(
                        EventType::NotificationFeedback(Feedback::Activated { serial, button }),
                        window,
                    )
                    .with_pane(pane),
                );
                Ok(())
            },
        ))?;
        let (proxy, event) = (self.proxy.clone(), self.event(Feedback::Closed(serial)));
        let dismissed = toast.Dismissed(&TypedEventHandler::new(move |_, _| {
            let _ = proxy.send_event(event.clone());
            Ok(())
        }))?;
        let (proxy, event) = (self.proxy.clone(), self.event(Feedback::Failed(serial)));
        let failed = toast.Failed(&TypedEventHandler::new(move |_, _| {
            let _ = proxy.send_event(event.clone());
            Ok(())
        }))?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?
            .Show(&toast)?;
        self.active.insert(serial, Toast { notification: toast, activated, dismissed, failed });
        Ok(())
    }

    pub fn close(&mut self, serial: u64) {
        if let Some(toast) = self.active.remove(&serial) {
            let _ = toast.notification.RemoveActivated(toast.activated);
            let _ = toast.notification.RemoveDismissed(toast.dismissed);
            let _ = toast.notification.RemoveFailed(toast.failed);
            if let Ok(notifier) =
                ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
            {
                let _ = notifier.Hide(&toast.notification);
            }
            if let Ok(history) = ToastNotificationManager::History() {
                let _ = history.RemoveGroupedTagWithId(
                    &HSTRING::from(format!("{serial:x}")),
                    &self.group,
                    &HSTRING::from(APP_ID),
                );
            }
        }
    }

    pub fn query_alive(&self, query: String) {
        let serials = ToastNotificationManager::History()
            .and_then(|history| history.GetHistoryWithId(&HSTRING::from(APP_ID)))
            .map(|history| {
                history
                    .into_iter()
                    .filter(|toast| toast.Group().is_ok_and(|group| group == self.group))
                    .filter_map(|toast| {
                        u64::from_str_radix(&toast.Tag().ok()?.to_string(), 16).ok()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let _ = self.proxy.send_event(self.event(Feedback::Alive { query, serials }));
    }
}

impl Drop for NativeNotifications {
    fn drop(&mut self) {
        for serial in self.active.keys().copied().collect::<Vec<_>>() {
            self.close(serial);
        }
    }
}
