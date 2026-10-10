use super::*;

impl ListenerState {
    pub(super) fn start_iterm2_multipart(&self, metadata: &str) {
        let (suggested_name, inline) = iterm2_file_metadata(metadata);
        if inline {
            return;
        }
        if let Some(previous) = self.iterm2_multipart.lock().take() {
            let _ = fs::remove_file(previous.temp_path);
        }
        let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
        let temp_path =
            std::env::temp_dir().join(format!("eggie-transfer-{}-{request_id}", self.session_id));
        let Ok(file) = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
        else {
            return;
        };
        *self.iterm2_multipart.lock() = Some(Iterm2MultipartTransfer {
            request_id,
            suggested_name,
            temp_path,
            file,
            size: 0,
        });
    }

    pub(super) fn append_iterm2_multipart(&self, encoded: &str) {
        let mut multipart = self.iterm2_multipart.lock();
        let Some(transfer) = multipart.as_mut() else {
            return;
        };
        let mut encoded = encoded.as_bytes();
        let decoder = DecoderReader::new(&mut encoded, &BASE64);
        match std::io::copy(
            &mut decoder.take(MAX_ITERM2_TRANSFER_BYTES + 1),
            &mut transfer.file,
        ) {
            Ok(size) if transfer.size.saturating_add(size) <= MAX_ITERM2_TRANSFER_BYTES => {
                transfer.size = transfer.size.saturating_add(size);
            }
            Err(_) => {
                let transfer = multipart
                    .take()
                    .expect("iTerm2 multipart transfer is present");
                let _ = fs::remove_file(transfer.temp_path);
            }
            Ok(_) => {
                let transfer = multipart
                    .take()
                    .expect("iTerm2 multipart transfer is present");
                let _ = fs::remove_file(transfer.temp_path);
            }
        }
    }

    pub(super) fn finish_iterm2_multipart(&self) {
        let Some(transfer) = self.iterm2_multipart.lock().take() else {
            return;
        };
        if transfer.file.sync_all().is_err() {
            let _ = fs::remove_file(transfer.temp_path);
            return;
        }
        self.pending_file_transfers.lock().insert(
            transfer.request_id,
            PendingFileTransfer::Iterm2 {
                temp_path: transfer.temp_path,
            },
        );
        self.osc_events.push(TerminalOscEventPayload::FileTransfer {
            offer: TerminalFileTransferOffer {
                request_id: transfer.request_id,
                protocol: TerminalFileTransferProtocol::Iterm2,
                suggested_name: transfer.suggested_name,
                size: transfer.size,
                directory: false,
            },
        });
    }

    pub(super) fn handle_iterm2_file(&self, payload: &str) {
        let Some(value) = payload.strip_prefix("File=") else {
            return;
        };
        let Some((metadata, encoded)) = value.split_once(':') else {
            return;
        };
        let (suggested_name, inline) = iterm2_file_metadata(metadata);
        if inline || encoded.is_empty() {
            return;
        }
        let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
        let temp_path =
            std::env::temp_dir().join(format!("eggie-transfer-{}-{request_id}", self.session_id));
        let Ok(mut file) = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
        else {
            return;
        };
        let mut encoded = encoded.as_bytes();
        let mut decoder = DecoderReader::new(&mut encoded, &BASE64);
        let Ok(size) = std::io::copy(&mut decoder, &mut file) else {
            let _ = fs::remove_file(&temp_path);
            return;
        };
        if file.sync_all().is_err() {
            let _ = fs::remove_file(&temp_path);
            return;
        }
        let mut pending = self.pending_file_transfers.lock();
        while pending.len() >= 16 {
            let Some(oldest) = pending.keys().copied().min() else {
                break;
            };
            if let Some(old) = pending.remove(&oldest) {
                match old {
                    PendingFileTransfer::Iterm2 { temp_path } => {
                        let _ = fs::remove_file(temp_path);
                    }
                    PendingFileTransfer::Kitty { transfer_id } => {
                        self.kitty_file_transfers.lock().remove(&transfer_id);
                    }
                }
            }
        }
        pending.insert(request_id, PendingFileTransfer::Iterm2 { temp_path });
        drop(pending);
        self.osc_events.push(TerminalOscEventPayload::FileTransfer {
            offer: TerminalFileTransferOffer {
                request_id,
                protocol: TerminalFileTransferProtocol::Iterm2,
                suggested_name,
                size,
                directory: false,
            },
        });
    }

    pub(super) fn complete_file_transfer(
        &self,
        request_id: u64,
        destination: Option<PathBuf>,
    ) -> Result<()> {
        let Some(pending) = self.pending_file_transfers.lock().remove(&request_id) else {
            bail!("unknown or expired terminal file-transfer request {request_id}");
        };
        let temp_path = match pending {
            PendingFileTransfer::Kitty { transfer_id } => {
                return self.complete_kitty_file_transfer(&transfer_id, destination);
            }
            PendingFileTransfer::Iterm2 { temp_path } => temp_path,
        };
        let Some(destination) = destination else {
            let _ = fs::remove_file(temp_path);
            return Ok(());
        };
        let copy_result = (|| -> Result<()> {
            let mut input = fs::File::open(&temp_path).with_context(|| {
                format!("failed to open pending transfer {}", temp_path.display())
            })?;
            let mut output = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&destination)
                .with_context(|| format!("failed to create {}", destination.display()))?;
            std::io::copy(&mut input, &mut output)
                .and_then(|_| output.sync_all())
                .with_context(|| {
                    format!(
                        "failed to save terminal transfer to {}",
                        destination.display()
                    )
                })?;
            Ok(())
        })();
        let _ = fs::remove_file(&temp_path);
        copy_result
    }

    pub(super) fn handle_kitty_file_transfer(&self, payload: &str, terminator: &str) {
        let fields = payload
            .split(';')
            .filter_map(|field| field.split_once('='))
            .collect::<HashMap<_, _>>();
        let Some(action) = fields.get("ac").copied() else {
            return;
        };
        let Some(transfer_id) = fields
            .get("id")
            .map(|id| sanitize_protocol_id(id))
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        match action {
            "send" => {
                if self.kitty_file_transfers.lock().contains_key(&transfer_id) {
                    self.kitty_transfer_status(
                        &transfer_id,
                        None,
                        "EINVAL:Duplicate transfer",
                        None,
                        terminator,
                    );
                    return;
                }
                if self.kitty_file_transfers.lock().len() >= 16 {
                    self.kitty_transfer_status(
                        &transfer_id,
                        None,
                        "EBUSY:Too many transfers",
                        None,
                        terminator,
                    );
                    return;
                }
                let request_id = self.next_osc_request_id.fetch_add(1, Ordering::Relaxed);
                self.kitty_file_transfers.lock().insert(
                    transfer_id.clone(),
                    KittyIncomingTransfer {
                        destination: None,
                        terminator: terminator.to_owned(),
                        files: HashMap::new(),
                    },
                );
                self.pending_file_transfers
                    .lock()
                    .insert(request_id, PendingFileTransfer::Kitty { transfer_id });
                self.osc_events.push(TerminalOscEventPayload::FileTransfer {
                    offer: TerminalFileTransferOffer {
                        request_id,
                        protocol: TerminalFileTransferProtocol::Kitty,
                        suggested_name: "Received Files".to_owned(),
                        size: 0,
                        directory: true,
                    },
                });
            }
            "file" => self.kitty_receive_file(&transfer_id, &fields),
            "data" | "end_data" => {
                self.kitty_receive_data(&transfer_id, &fields, action == "end_data")
            }
            "finish" | "finished" => {
                if let Some(mut transfer) = self.kitty_file_transfers.lock().remove(&transfer_id) {
                    let mut failed = false;
                    for file in transfer.files.values_mut() {
                        if let Some(sink) = file.sink.take() {
                            match sink
                                .finish()
                                .and_then(|handle| handle.sync_all().map(|_| handle))
                            {
                                Ok(handle) => {
                                    file.size = handle
                                        .metadata()
                                        .map_or(file.size, |metadata| metadata.len());
                                    if let Some(mode) = file.permissions {
                                        let _ = fs::set_permissions(
                                            &file.path,
                                            fs::Permissions::from_mode(mode & 0o777),
                                        );
                                    }
                                    failed |=
                                        file.expected_size.is_some_and(|size| size != file.size);
                                }
                                Err(_) => failed = true,
                            }
                        }
                    }
                    self.remove_pending_kitty_transfer(&transfer_id);
                    self.kitty_transfer_status(
                        &transfer_id,
                        None,
                        if failed {
                            "EIO:Incomplete transfer"
                        } else {
                            "OK"
                        },
                        None,
                        &transfer.terminator,
                    );
                }
            }
            "cancel" => {
                let terminator = self
                    .kitty_file_transfers
                    .lock()
                    .remove(&transfer_id)
                    .map_or_else(
                        || terminator.to_owned(),
                        |transfer| {
                            let terminator = transfer.terminator.clone();
                            cleanup_kitty_transfer(transfer);
                            terminator
                        },
                    );
                self.remove_pending_kitty_transfer(&transfer_id);
                self.kitty_transfer_status(&transfer_id, None, "CANCELED", None, &terminator);
            }
            _ => {}
        }
    }

    pub(super) fn complete_kitty_file_transfer(
        &self,
        transfer_id: &str,
        destination: Option<PathBuf>,
    ) -> Result<()> {
        let mut transfers = self.kitty_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(transfer_id) else {
            bail!("unknown or expired Kitty file-transfer session {transfer_id}");
        };
        let terminator = transfer.terminator.clone();
        if let Some(destination) = destination {
            if !destination.is_dir() {
                bail!("Kitty file-transfer destination is not a directory");
            }
            transfer.destination = Some(destination);
            drop(transfers);
            self.kitty_transfer_status(transfer_id, None, "OK", None, &terminator);
        } else {
            transfers.remove(transfer_id);
            drop(transfers);
            self.kitty_transfer_status(
                transfer_id,
                None,
                "EPERM:User refused the transfer",
                None,
                &terminator,
            );
        }
        Ok(())
    }

    pub(super) fn kitty_receive_file(&self, transfer_id: &str, fields: &HashMap<&str, &str>) {
        let Some(file_id) = fields
            .get("fid")
            .map(|id| sanitize_protocol_id(id))
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        let mut transfers = self.kitty_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(transfer_id) else {
            return;
        };
        let Some(destination) = transfer.destination.as_ref() else {
            return;
        };
        let terminator = transfer.terminator.clone();
        if transfer.files.len() >= 4_096 {
            drop(transfers);
            self.kitty_transfer_status(
                transfer_id,
                Some(&file_id),
                "EFBIG:Too many files",
                None,
                &terminator,
            );
            return;
        }
        let relative_path = safe_kitty_transfer_path(fields.get("n").copied(), &file_id);
        let path = destination.join(relative_path);
        let file_type = fields.get("ft").copied().unwrap_or("regular");
        let result = match file_type {
            "directory" => fs::create_dir_all(&path).map(|_| None),
            "regular" => {
                let create_parent = path.parent().map_or(Ok(()), fs::create_dir_all);
                create_parent
                    .and_then(|_| {
                        OpenOptions::new()
                            .create(true)
                            .truncate(true)
                            .write(true)
                            .open(&path)
                    })
                    .map(|file| {
                        Some(if fields.get("zip").copied() == Some("zlib") {
                            KittyIncomingSink::Zlib(ZlibDecoder::new(file))
                        } else {
                            KittyIncomingSink::Plain(file)
                        })
                    })
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "unsupported file type",
            )),
        };
        match result {
            Ok(file) => {
                transfer.files.insert(
                    file_id.clone(),
                    KittyIncomingFile {
                        path,
                        sink: file,
                        size: 0,
                        expected_size: fields.get("sz").and_then(|size| size.parse().ok()),
                        permissions: fields.get("prm").and_then(|mode| mode.parse().ok()),
                        directory: file_type == "directory",
                    },
                );
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    if file_type == "directory" {
                        "OK"
                    } else {
                        "STARTED"
                    },
                    Some(0),
                    &terminator,
                );
            }
            Err(error) => {
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    &format!("EIO:{error}"),
                    None,
                    &terminator,
                );
            }
        }
    }

    pub(super) fn kitty_receive_data(
        &self,
        transfer_id: &str,
        fields: &HashMap<&str, &str>,
        finished: bool,
    ) {
        let Some(file_id) = fields
            .get("fid")
            .map(|id| sanitize_protocol_id(id))
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        let mut transfers = self.kitty_file_transfers.lock();
        let Some(transfer) = transfers.get_mut(transfer_id) else {
            return;
        };
        let terminator = transfer.terminator.clone();
        let data = match fields.get("d").map(|data| BASE64.decode(data)) {
            Some(Ok(data)) if data.len() <= KITTY_CLIPBOARD_CHUNK_BYTES => data,
            Some(Ok(_)) => {
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    "EINVAL:Data chunk exceeds 4096 bytes",
                    None,
                    &terminator,
                );
                return;
            }
            Some(Err(_)) => {
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    "EINVAL:Invalid base64 data",
                    None,
                    &terminator,
                );
                return;
            }
            None if finished => Vec::new(),
            None => {
                drop(transfers);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    "EINVAL:Missing data",
                    None,
                    &terminator,
                );
                return;
            }
        };
        let Some(file) = transfer.files.get_mut(&file_id) else {
            drop(transfers);
            self.kitty_transfer_status(
                transfer_id,
                Some(&file_id),
                "EINVAL:Unknown file",
                None,
                &terminator,
            );
            return;
        };
        let result = file
            .sink
            .as_mut()
            .ok_or_else(|| std::io::Error::other("file has no data stream"))
            .and_then(|sink| sink.write_chunk(&data));
        if let Ok(written) = result {
            file.size = file.size.saturating_add(written);
            if finished {
                let finish = file
                    .sink
                    .take()
                    .expect("Kitty data stream exists after successful write")
                    .finish()
                    .and_then(|handle| handle.sync_all().map(|_| handle));
                match finish {
                    Ok(handle) => {
                        file.size = handle
                            .metadata()
                            .map_or(file.size, |metadata| metadata.len());
                        if let Some(mode) = file.permissions {
                            let _ = fs::set_permissions(
                                &file.path,
                                fs::Permissions::from_mode(mode & 0o777),
                            );
                        }
                    }
                    Err(error) => {
                        let path = file.path.clone();
                        let size = file.size;
                        drop(transfers);
                        let _ = fs::remove_file(path);
                        self.kitty_transfer_status(
                            transfer_id,
                            Some(&file_id),
                            &format!("EIO:{error}"),
                            Some(size),
                            &terminator,
                        );
                        return;
                    }
                }
            }
        }
        let size = file.size;
        let wrong_size = finished && file.expected_size.is_some_and(|expected| expected != size);
        let path = file.path.clone();
        drop(transfers);
        if wrong_size {
            let _ = fs::remove_file(&path);
        }
        match result {
            Ok(_) => self.kitty_transfer_status(
                transfer_id,
                Some(&file_id),
                if wrong_size {
                    "EIO:Size mismatch"
                } else if finished {
                    "OK"
                } else {
                    "PROGRESS"
                },
                Some(size),
                &terminator,
            ),
            Err(error) => {
                let _ = fs::remove_file(path);
                self.kitty_transfer_status(
                    transfer_id,
                    Some(&file_id),
                    &format!("EIO:{error}"),
                    Some(size),
                    &terminator,
                );
            }
        }
    }

    pub(super) fn remove_pending_kitty_transfer(&self, transfer_id: &str) {
        self.pending_file_transfers.lock().retain(|_, pending| {
            !matches!(pending, PendingFileTransfer::Kitty { transfer_id: pending_id } if pending_id == transfer_id)
        });
    }

    pub(super) fn kitty_transfer_status(
        &self,
        transfer_id: &str,
        file_id: Option<&str>,
        status: &str,
        size: Option<u64>,
        terminator: &str,
    ) {
        let mut response = format!(
            "\x1b]5113;ac=status;id={transfer_id};st={}",
            BASE64.encode(status.as_bytes())
        );
        if let Some(file_id) = file_id {
            response.push_str(";fid=");
            response.push_str(file_id);
        }
        if let Some(size) = size {
            use std::fmt::Write as _;
            let _ = write!(response, ";sz={size}");
        }
        response.push_str(terminator);
        self.write_to_pty(response.into_bytes());
    }
}
