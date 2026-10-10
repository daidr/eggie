use super::*;

impl ListenerState {
    pub(super) fn clipboard_store(&self, clipboard: ClipboardType, text: String) {
        self.osc_events
            .push(TerminalOscEventPayload::ClipboardWrite {
                selection: clipboard_selection(clipboard),
                contents: vec![TerminalClipboardContent {
                    mime_type: "text/plain;charset=utf-8".to_owned(),
                    data: text.into_bytes(),
                }],
            });
    }

    pub(super) fn clipboard_load(
        &self,
        clipboard: ClipboardType,
        formatter: Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ) {
        if !self.allow_clipboard_read.load(Ordering::Acquire) {
            return;
        }
        let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
        self.pending_clipboard_reads
            .lock()
            .insert(request_id, ClipboardReadResponder::Osc52(formatter));
        self.osc_events
            .push(TerminalOscEventPayload::ClipboardRead {
                request_id,
                selection: clipboard_selection(clipboard),
                mime_types: vec!["text/plain;charset=utf-8".to_owned()],
            });
    }

    pub(super) fn complete_clipboard_read(
        &self,
        request_id: u64,
        contents: Vec<TerminalClipboardContent>,
    ) {
        let Some(responder) = self.pending_clipboard_reads.lock().remove(&request_id) else {
            return;
        };
        match responder {
            ClipboardReadResponder::Osc52(formatter) => {
                let text = contents
                    .iter()
                    .find(|content| content.mime_type.starts_with("text/plain"))
                    .and_then(|content| String::from_utf8(content.data.clone()).ok())
                    .unwrap_or_default();
                self.write_to_pty(formatter(&text).into_bytes());
            }
            ClipboardReadResponder::Kitty {
                id,
                terminator,
                selection,
                list_only,
            } => self.send_kitty_clipboard_read_response(
                &id,
                selection,
                list_only,
                &contents,
                &terminator,
            ),
        }
    }

    pub(super) fn send_kitty_clipboard_read_response(
        &self,
        id: &str,
        selection: TerminalClipboardSelection,
        list_only: bool,
        contents: &[TerminalClipboardContent],
        terminator: &str,
    ) {
        let id = protocol_id_field(id);
        let location = if selection == TerminalClipboardSelection::Primary {
            ":loc=primary"
        } else {
            ""
        };
        if !list_only && contents.is_empty() {
            self.write_to_pty(
                format!("\x1b]5522;type=read:status=ENOSYS{location}{id}{terminator}").into_bytes(),
            );
            return;
        }
        self.write_to_pty(
            format!("\x1b]5522;type=read:status=OK{location}{id}{terminator}").into_bytes(),
        );
        let mut seen_mime_types = HashSet::new();
        for content in contents {
            if !seen_mime_types.insert(content.mime_type.as_str()) {
                continue;
            }
            let mime = BASE64.encode(content.mime_type.as_bytes());
            if list_only || content.data.is_empty() {
                self.write_to_pty(
                    format!("\x1b]5522;type=read:status=DATA{id}:mime={mime}{terminator}")
                        .into_bytes(),
                );
                continue;
            }
            for chunk in content.data.chunks(KITTY_CLIPBOARD_CHUNK_BYTES) {
                let payload = BASE64.encode(chunk);
                self.write_to_pty(
                    format!(
                        "\x1b]5522;type=read:status=DATA{id}:mime={mime};{payload}{terminator}"
                    )
                    .into_bytes(),
                );
            }
        }
        self.write_to_pty(format!("\x1b]5522;type=read:status=DONE{id}{terminator}").into_bytes());
    }

    pub(super) fn paste_clipboard(
        &self,
        selection: TerminalClipboardSelection,
        contents: Vec<TerminalClipboardContent>,
    ) {
        let now = Instant::now();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut grants = self.paste_clipboard_grants.lock();
        grants.retain(|_, grant| grant.expires_at > now);
        while grants.len() >= 16 {
            let Some(oldest) = grants
                .iter()
                .min_by_key(|(_, grant)| grant.expires_at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            grants.remove(&oldest);
        }
        grants.insert(
            token.clone(),
            PasteClipboardGrant {
                selection,
                contents: contents.clone(),
                expires_at: now + PASTE_CLIPBOARD_GRANT_TTL,
            },
        );
        drop(grants);

        let location = if selection == TerminalClipboardSelection::Primary {
            ":loc=primary"
        } else {
            ""
        };
        self.write_to_pty(
            format!(
                "\x1b]5522;type=read:status=OK{location}:pw={}\x1b\\",
                BASE64.encode(token.as_bytes())
            )
            .into_bytes(),
        );
        let mut seen = HashSet::new();
        for content in contents {
            if seen.insert(content.mime_type.clone()) {
                self.write_to_pty(
                    format!(
                        "\x1b]5522;type=read:status=DATA:mime={}\x1b\\",
                        BASE64.encode(content.mime_type.as_bytes())
                    )
                    .into_bytes(),
                );
            }
        }
        self.write_to_pty(b"\x1b]5522;type=read:status=DONE\x1b\\".to_vec());
    }

    pub(super) fn handle_kitty_clipboard(&self, payload: &str, terminator: &str) {
        let (metadata, encoded_payload) = payload.split_once(';').unwrap_or((payload, ""));
        let fields = colon_metadata(metadata);
        let kind = fields.get("type").map_or("", String::as_str);
        let id = fields
            .get("id")
            .map_or_else(String::new, |id| sanitize_protocol_id(id));
        let selection = if fields.get("loc").is_some_and(|value| value == "primary") {
            TerminalClipboardSelection::Primary
        } else {
            TerminalClipboardSelection::Clipboard
        };
        match kind {
            "write" => {
                *self.rich_clipboard_write.lock() = Some(RichClipboardWrite {
                    id,
                    selection,
                    contents: Vec::new(),
                    aliases: Vec::new(),
                    total_size: 0,
                    terminator: terminator.to_owned(),
                });
            }
            "wdata" => {
                let mut pending = self.rich_clipboard_write.lock();
                let Some(write) = pending.as_mut() else {
                    return;
                };
                if !id.is_empty() && id != write.id {
                    return;
                }
                if let Some(mime) = fields.get("mime").and_then(|mime| decode_base64_text(mime)) {
                    if mime.len() > 1_024
                        || (!write
                            .contents
                            .iter()
                            .any(|content| content.mime_type == mime)
                            && write.contents.len() >= 64)
                    {
                        let failed = pending.take().expect("rich clipboard write is present");
                        self.write_to_pty(
                            format!(
                                "\x1b]5522;type=write:status=EIO{}{}",
                                protocol_id_field(&failed.id),
                                failed.terminator
                            )
                            .into_bytes(),
                        );
                        return;
                    }
                    let Ok(data) = BASE64.decode(encoded_payload) else {
                        let failed = pending.take().expect("rich clipboard write is present");
                        self.write_to_pty(
                            format!(
                                "\x1b]5522;type=write:status=EINVAL{}{}",
                                protocol_id_field(&failed.id),
                                failed.terminator
                            )
                            .into_bytes(),
                        );
                        return;
                    };
                    if data.len() > KITTY_CLIPBOARD_CHUNK_BYTES
                        || write.total_size.saturating_add(data.len()) > MAX_RICH_CLIPBOARD_BYTES
                    {
                        let failed = pending.take().expect("rich clipboard write is present");
                        self.write_to_pty(
                            format!(
                                "\x1b]5522;type=write:status=EIO{}{}",
                                protocol_id_field(&failed.id),
                                failed.terminator
                            )
                            .into_bytes(),
                        );
                        return;
                    }
                    write.total_size += data.len();
                    if let Some(existing) = write
                        .contents
                        .iter_mut()
                        .find(|content| content.mime_type == mime)
                    {
                        existing.data.extend(data);
                    } else {
                        write.contents.push(TerminalClipboardContent {
                            mime_type: mime,
                            data,
                        });
                    }
                } else {
                    let mut write = pending.take().expect("rich clipboard write is present");
                    let mut committed_size = write.total_size;
                    for (target, alias) in &write.aliases {
                        if write
                            .contents
                            .iter()
                            .any(|content| content.mime_type == *alias)
                        {
                            continue;
                        }
                        if let Some(content) = write
                            .contents
                            .iter()
                            .find(|content| content.mime_type == *target)
                            .cloned()
                        {
                            if committed_size.saturating_add(content.data.len())
                                > MAX_RICH_CLIPBOARD_BYTES
                            {
                                continue;
                            }
                            committed_size += content.data.len();
                            write.contents.push(TerminalClipboardContent {
                                mime_type: alias.clone(),
                                data: content.data,
                            });
                        }
                    }
                    self.osc_events
                        .push(TerminalOscEventPayload::ClipboardWrite {
                            selection: write.selection,
                            contents: write.contents,
                        });
                    self.write_to_pty(
                        format!(
                            "\x1b]5522;type=write:status=DONE{}{}",
                            protocol_id_field(&write.id),
                            write.terminator
                        )
                        .into_bytes(),
                    );
                }
            }
            "walias" => {
                let Some(target) = fields.get("mime").and_then(|mime| decode_base64_text(mime))
                else {
                    return;
                };
                let aliases = BASE64
                    .decode(encoded_payload)
                    .ok()
                    .and_then(|aliases| String::from_utf8(aliases).ok())
                    .unwrap_or_default();
                if let Some(write) = self.rich_clipboard_write.lock().as_mut() {
                    if !id.is_empty() && id != write.id {
                        return;
                    }
                    for alias in aliases.split_whitespace().take(32) {
                        if alias.len() <= 1_024 && write.aliases.len() < 64 {
                            write.aliases.push((target.clone(), alias.to_owned()));
                        }
                    }
                }
            }
            "read" => {
                let requested = BASE64
                    .decode(encoded_payload)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .or_else(|| fields.get("mime").and_then(|mime| decode_base64_text(mime)))
                    .unwrap_or_else(|| "text/plain;charset=utf-8".to_owned());
                let list_only = requested.trim() == ".";
                let mime_types = if list_only {
                    vec![".".to_owned()]
                } else {
                    requested
                        .split_whitespace()
                        .filter(|mime| mime.len() <= 1_024)
                        .take(64)
                        .map(str::to_owned)
                        .collect()
                };
                let authorized_contents = fields
                    .get("pw")
                    .and_then(|password| decode_base64_text(password))
                    .and_then(|password| {
                        let now = Instant::now();
                        let mut grants = self.paste_clipboard_grants.lock();
                        grants.retain(|_, grant| grant.expires_at > now);
                        grants.remove(&password).and_then(|grant| {
                            (grant.selection == selection).then_some(grant.contents)
                        })
                    });
                if let Some(contents) = authorized_contents {
                    let filtered = filter_clipboard_contents(contents, &mime_types);
                    self.send_kitty_clipboard_read_response(
                        &id, selection, false, &filtered, terminator,
                    );
                    return;
                }
                if !list_only && !self.allow_clipboard_read.load(Ordering::Acquire) {
                    self.write_to_pty(
                        format!(
                            "\x1b]5522;type=read:status=EPERM{}{terminator}",
                            protocol_id_field(&id)
                        )
                        .into_bytes(),
                    );
                    return;
                }
                let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
                self.pending_clipboard_reads.lock().insert(
                    request_id,
                    ClipboardReadResponder::Kitty {
                        id,
                        terminator: terminator.to_owned(),
                        selection,
                        list_only,
                    },
                );
                self.osc_events
                    .push(TerminalOscEventPayload::ClipboardRead {
                        request_id,
                        selection,
                        mime_types,
                    });
            }
            _ => {}
        }
    }
}
