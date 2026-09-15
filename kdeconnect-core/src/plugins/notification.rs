use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Notification {
    pub id: Option<String>,
    pub title: Option<String>,
    pub text: Option<String>,
    pub ticker: Option<String>,
    #[serde(rename = "appName")]
    pub app_name: Option<String>,
    #[serde(rename = "isClearable")]
    pub is_clearable: Option<bool>,
    pub silent: Option<bool>,
    #[serde(rename = "requestReplyId")]
    pub request_reply_id: Option<String>,
    pub time: Option<String>,
    pub actions: Option<Vec<String>>,
    #[serde(rename = "payloadHash")]
    pub payload_hash: Option<String>,
}

impl Notification {
    pub async fn received_packet(
        &self,
        _device: &crate::device::Device,
        _core_event: mpsc::UnboundedSender<crate::event::CoreEvent>,
    ) {
        let app_name = self.app_name.clone().unwrap_or_default();
        let Some(title) = self.title.clone() else {
            return;
        };
        let Some(text) = self.text.clone() else {
            return;
        };
        let actions = self.actions.clone().unwrap_or_default();

        // Notification replies (NotificationAction) require wait_for_action,
        // which COSMIC's notification daemon does not support — so actions
        // are shown but a click can't be reported back to the phone.
        let _ = tokio::task::spawn_blocking(move || {
            let mut notify = notify_rust::Notification::new();
            notify.appname(&app_name);
            notify.summary(&title);
            notify.body(&text);

            for action in actions.iter() {
                notify.action(action, action);
            }

            notify
                .hint(notify_rust::Hint::Resident(true))
                .show()
                .unwrap();
        })
        .await;
    }
}
