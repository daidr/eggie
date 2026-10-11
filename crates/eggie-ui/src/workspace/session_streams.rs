//! Subscribe once per process, so window moves cannot replay OSC commands or lose background progress.
use super::*;

impl WindowManager {
    pub fn ensure_streams(&mut self, client: DaemonClient, cx: &mut Context<Self>) {
        for id in self
            .sessions
            .iter()
            .map(|session| session.id)
            .collect::<Vec<_>>()
        {
            if self.streams.insert(id) {
                self.stream_liveness.insert(
                    id,
                    std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
                );
                self.watch_stream(id, false, client.clone(), cx);
                self.watch_stream(id, true, client.clone(), cx);
            }
        }
    }

    fn watch_stream(
        &self,
        id: SessionId,
        progress: bool,
        client: DaemonClient,
        cx: &mut Context<Self>,
    ) {
        let (mut sender, mut receiver) = futures::channel::mpsc::channel(1);
        let alive = self.stream_liveness[&id].clone();
        let delivery_alive = alive.clone();
        let worker_client = client.clone();
        let initial_revision = if progress {
            0
        } else {
            self.state.osc_cursors.get(&id).copied().unwrap_or(0)
        };
        std::thread::Builder::new()
            .name(format!(
                "eggie-{}-{id}",
                if progress { "progress" } else { "osc" }
            ))
            .spawn(move || {
                use futures::SinkExt;
                let mut revision = initial_revision;
                let mut connection = None;
                while alive.load(std::sync::atomic::Ordering::Acquire) {
                    let result = (|| {
                        if connection.is_none() {
                            connection = Some(worker_client.connect()?);
                        }
                        let request = if progress {
                            ClientRequest::WaitForProgress {
                                session_id: id,
                                after_revision: revision,
                                timeout_ms: 1000,
                            }
                        } else {
                            ClientRequest::WaitForOscEvents {
                                session_id: id,
                                after_revision: revision,
                                timeout_ms: 1000,
                            }
                        };
                        connection.as_mut().unwrap().request(request)
                    })();
                    match &result {
                        Ok(DaemonResponse::Progress { update }) => revision = update.revision,
                        Ok(DaemonResponse::OscEvents { update }) => revision = update.revision,
                        Ok(DaemonResponse::ProgressUnchanged { revision: next, .. })
                        | Ok(DaemonResponse::OscEventsUnchanged { revision: next, .. }) => {
                            revision = revision.max(*next)
                        }
                        Err(_) => connection = None,
                        _ => {}
                    }
                    let failed = result.is_err();
                    if futures::executor::block_on(sender.send(result)).is_err() {
                        break;
                    }
                    if failed {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            })
            .expect("failed to start terminal event worker");
        cx.spawn(async move |manager, cx| {
            use futures::StreamExt;
            while let Some(result) = receiver.next().await {
                let mut events = Vec::new();
                let updated = manager
                    .update(cx, |manager, cx| {
                        if !delivery_alive.load(std::sync::atomic::Ordering::Acquire)
                            || !manager.sessions.iter().any(|s| s.id == id)
                        {
                            return false;
                        }
                        match result {
                            Ok(DaemonResponse::Progress { update }) if update.session_id == id => {
                                manager.progress.insert(id, update);
                                manager.schedule_stream_frame(cx);
                            }
                            Ok(DaemonResponse::ProgressUnchanged { .. })
                            | Ok(DaemonResponse::OscEventsUnchanged { .. }) => {}
                            Ok(DaemonResponse::OscEvents { update }) if update.session_id == id => {
                                manager.state.osc_cursors.insert(id, update.revision);
                                manager.schedule_save(cx);
                                events = update
                                    .events
                                    .into_iter()
                                    .map(|event| event.payload)
                                    .collect();
                            }
                            Err(error) => {
                                eprintln!("terminal event stream failed for {id}: {error:#}")
                            }
                            _ => {}
                        }
                        true
                    })
                    .unwrap_or(false);
                if !updated {
                    break;
                }
                if !events.is_empty() {
                    let _ = cx.update(|cx| {
                        let Some(manager) = manager.upgrade() else {
                            return;
                        };
                        // Resolve the destination when delivering, never when subscribing.
                        let registry = manager.read(cx);
                        let handle = registry
                            .state
                            .locate(id)
                            .and_then(|p| registry.handles.get(&p.window_id).copied())
                            .or_else(|| {
                                registry
                                    .state
                                    .active_window
                                    .and_then(|id| registry.handles.get(&id).copied())
                            })
                            .or_else(|| registry.handles.values().next().copied());
                        let routes = registry.notification_routes.clone();
                        for event in events {
                            if let Some(handle) = handle {
                                let _ = handle.update(cx, |app, window, cx| {
                                    app.handle_osc_event(id, event, window, cx)
                                });
                            } else {
                                crate::app::EggieApp::deliver_osc_without_window(
                                    id,
                                    event,
                                    client.clone(),
                                    &routes,
                                    cx,
                                );
                            }
                        }
                    });
                }
            }
        })
        .detach();
    }

    fn schedule_stream_frame(&mut self, cx: &mut Context<Self>) {
        if self.stream_notify_scheduled {
            return;
        }
        self.stream_notify_scheduled = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |manager, cx| {
            executor.timer(Duration::from_millis(16)).await;
            let _ = manager.update(cx, |manager, cx| {
                manager.stream_notify_scheduled = false;
                cx.notify();
            });
        })
        .detach();
    }
}
