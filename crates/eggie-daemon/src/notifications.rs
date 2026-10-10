use super::*;

impl ListenerState {
    pub(super) fn handle_notification(
        &self,
        notification: alacritty_terminal::vte::ansi::DesktopNotification,
    ) {
        match notification.code {
            9 => self.publish_notification(ReadyNotification {
                id: generated_notification_id(self.session_id, &self.next_osc_request_id),
                protocol_id: "0".to_owned(),
                title: notification.payload,
                body: String::new(),
                activation_response_requested: false,
                close_response_requested: false,
                terminator: notification.terminator,
            }),
            777 => {
                let Some(payload) = notification.payload.strip_prefix("notify;") else {
                    return;
                };
                let (title, body) = payload.split_once(';').unwrap_or((payload, ""));
                self.publish_notification(ReadyNotification {
                    id: generated_notification_id(self.session_id, &self.next_osc_request_id),
                    protocol_id: "0".to_owned(),
                    title: title.to_owned(),
                    body: body.to_owned(),
                    activation_response_requested: false,
                    close_response_requested: false,
                    terminator: notification.terminator,
                });
            }
            99 => self.handle_kitty_notification(&notification.payload, notification.terminator),
            _ => {}
        }
    }

    pub(super) fn handle_kitty_notification(&self, payload: &str, terminator: String) {
        let (metadata, payload) = payload.split_once(';').unwrap_or((payload, ""));
        let fields = colon_metadata(metadata);
        let explicit_id = fields
            .get("i")
            .filter(|id| !id.is_empty())
            .map(|id| sanitize_protocol_id(id))
            .filter(|id| !id.is_empty());
        let protocol_id = explicit_id.clone().unwrap_or_else(|| "0".to_owned());
        let id = explicit_id.clone().unwrap_or_else(|| {
            generated_notification_id(self.session_id, &self.next_osc_request_id)
        });
        let payload_type = fields.get("p").map_or("title", String::as_str);
        if payload_type == "?" {
            self.write_to_pty(
                format!(
                    "\x1b]99;i={protocol_id}:p=?;a=focus,report:o=unfocused,invisible:p=title,body,?,close,alive:c=1{terminator}"
                )
                .into_bytes(),
            );
            return;
        }
        if payload_type == "close" {
            let Some(id) = explicit_id else {
                return;
            };
            self.osc_events
                .push(TerminalOscEventPayload::NotificationClose { id: id.clone() });
            self.pending_notifications.lock().remove(&id);
            self.notification_replies.lock().remove(&id);
            self.live_notifications.lock().remove(&id);
            return;
        }
        if payload_type == "alive" {
            let mut live = self
                .live_notifications
                .lock()
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            live.sort_unstable();
            self.write_to_pty(
                format!(
                    "\x1b]99;i={protocol_id}:p=alive;{}{terminator}",
                    live.join(",")
                )
                .into_bytes(),
            );
            return;
        }

        let decoded = if fields.get("e").is_some_and(|value| value == "1") {
            BASE64
                .decode(payload)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default()
        } else {
            payload.to_owned()
        };
        let done = fields.get("d").is_none_or(|value| value != "0");
        let actions = fields.get("a").map_or("focus", String::as_str);
        let activation_response_requested = actions.split(',').any(|action| action == "report");
        let close_response_requested = fields.get("c").is_some_and(|value| value != "0");
        let mut pending = self.pending_notifications.lock();
        if !pending.contains_key(&id)
            && pending.len() >= MAX_PENDING_NOTIFICATIONS
            && let Some(expired) = pending.keys().next().cloned()
        {
            pending.remove(&expired);
        }
        let command = pending.entry(id.clone()).or_default();
        match payload_type {
            "body" => append_sanitized_osc_text(&mut command.body, &decoded, 4_096),
            "title" | "" => append_sanitized_osc_text(&mut command.title, &decoded, 1_024),
            _ => {}
        }
        command.activation_response_requested |= activation_response_requested;
        command.close_response_requested |= close_response_requested;
        if !done {
            return;
        }
        let command = pending.remove(&id).unwrap_or_default();
        drop(pending);
        self.publish_notification(ReadyNotification {
            id,
            protocol_id,
            title: command.title,
            body: command.body,
            activation_response_requested: command.activation_response_requested,
            close_response_requested: command.close_response_requested,
            terminator,
        });
    }

    pub(super) fn publish_notification(&self, notification: ReadyNotification) {
        let ReadyNotification {
            id,
            protocol_id,
            mut title,
            mut body,
            activation_response_requested,
            close_response_requested,
            terminator,
        } = notification;
        title = sanitize_osc_text(&title, 1_024);
        body = sanitize_osc_text(&body, 4_096);
        if title.is_empty() {
            std::mem::swap(&mut title, &mut body);
        }
        if title.is_empty() {
            return;
        }
        if protocol_id != "0" {
            let mut live = self.live_notifications.lock();
            if !live.contains(&protocol_id)
                && live.len() >= MAX_PENDING_NOTIFICATIONS
                && let Some(expired) = live.iter().next().cloned()
            {
                live.remove(&expired);
            }
            live.insert(protocol_id.clone());
        }
        let mut close_response_requested = close_response_requested;
        #[cfg(target_os = "macos")]
        if close_response_requested {
            // macOS does not reliably report Notification Center dismissal. Kitty's protocol
            // defines `untracked` specifically for this platform limitation.
            self.write_to_pty(
                format!("\x1b]99;i={protocol_id}:p=close;untracked{terminator}").into_bytes(),
            );
            close_response_requested = false;
        }
        if activation_response_requested || close_response_requested {
            let mut replies = self.notification_replies.lock();
            if !replies.contains_key(&id)
                && replies.len() >= MAX_PENDING_NOTIFICATIONS
                && let Some(expired) = replies.keys().next().cloned()
            {
                replies.remove(&expired);
            }
            replies.insert(
                id.clone(),
                NotificationReply {
                    protocol_id,
                    terminator,
                    activation_response_requested,
                    close_response_requested,
                },
            );
        }
        self.osc_events.push(TerminalOscEventPayload::Notification {
            notification: TerminalNotification {
                id,
                title,
                body,
                activation_response_requested,
                close_response_requested,
            },
        });
    }

    pub(super) fn notification_response(&self, notification_id: &str, activated: bool) {
        let mut replies = self.notification_replies.lock();
        let Some(reply) = replies.remove(notification_id) else {
            self.live_notifications.lock().remove(notification_id);
            return;
        };
        if activated && reply.activation_response_requested {
            self.write_to_pty(
                format!("\x1b]99;i={};{}", reply.protocol_id, reply.terminator).into_bytes(),
            );
        }
        if reply.close_response_requested {
            self.write_to_pty(
                format!(
                    "\x1b]99;i={}:p=close;{}",
                    reply.protocol_id, reply.terminator
                )
                .into_bytes(),
            );
        }
        self.live_notifications.lock().remove(&reply.protocol_id);
    }
}
