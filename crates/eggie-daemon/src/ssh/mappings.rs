use super::*;
use eggie_protocol::{
    PortForwardKind, PortMapping, PortMappingError, PortMappingErrorKind, PortMappingRequest,
    PortMappingState,
};
use std::net::TcpListener;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct SavedMapping {
    id: String,
    pub(super) request: PortMappingRequest,
    enabled: bool,
}
pub(super) struct MappingRecord {
    pub mapping: PortMapping,
    pub spec: Option<PortForwardSpec>,
    pub enabled: bool,
}
impl MappingRecord {
    pub fn restore(saved: SavedMapping) -> Self {
        Self {
            mapping: PortMapping {
                id: saved.id,
                local_port: saved.request.local_port,
                remote_port: saved.request.remote_port,
                request: saved.request,
                state: if saved.enabled {
                    PortMappingState::Disconnected
                } else {
                    PortMappingState::Stopped
                },
                error: None,
            },
            spec: None,
            enabled: saved.enabled,
        }
    }
}
fn mapping_error(detail: String, kind: PortMappingErrorKind) -> PortMappingError {
    PortMappingError {
        kind,
        detail: detail.chars().take(2048).collect(),
    }
}
fn error_from_output(output: &std::process::Output) -> PortMappingError {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let lower = detail.to_lowercase();
    let kind = if lower.contains("address already in use")
        || lower.contains("cannot listen to port")
    {
        PortMappingErrorKind::PortInUse
    } else if lower.contains("permission denied") || lower.contains("administratively prohibited") {
        PortMappingErrorKind::AccessDenied
    } else if lower.contains("control socket")
        || lower.contains("connection refused")
        || lower.contains("master is not running")
    {
        PortMappingErrorKind::ConnectionUnavailable
    } else {
        PortMappingErrorKind::RequestFailed
    };
    mapping_error(detail, kind)
}
fn target_address(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}
fn choose_local_port(requested: Option<u16>, preferred: u16) -> Result<u16, PortMappingError> {
    let listener = if let Some(port) = requested {
        TcpListener::bind(("127.0.0.1", port))
    } else {
        TcpListener::bind(("127.0.0.1", preferred)).or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
    }
    .map_err(|error| {
        mapping_error(
            error.to_string(),
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                PortMappingErrorKind::AccessDenied
            } else {
                PortMappingErrorKind::PortInUse
            },
        )
    })?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| mapping_error(e.to_string(), PortMappingErrorKind::RequestFailed))
}
impl SshConnection {
    pub fn has_enabled_mappings(&self) -> bool {
        self.mappings.lock().iter().any(|r| r.enabled)
    }
    fn persist_mappings(&self) -> Result<()> {
        let saved = self
            .mappings
            .lock()
            .iter()
            .filter(|r| r.mapping.request.remember)
            .map(|r| SavedMapping {
                id: r.mapping.id.clone(),
                request: r.mapping.request.clone(),
                enabled: r.enabled,
            })
            .collect();
        self.store.save(&self.target, saved)
    }
    fn persist_mapping_state(&self, id: &str) {
        let result = self.persist_mappings();
        let mut records = self.mappings.lock();
        if let Some(record) = records.iter_mut().find(|r| r.mapping.id == id) {
            if let Err(error) = result {
                record.mapping.error = Some(mapping_error(
                    error.to_string(),
                    PortMappingErrorKind::PersistenceFailed,
                ));
                self.revision.fetch_add(1, Ordering::Release);
            } else if record
                .mapping
                .error
                .as_ref()
                .is_some_and(|e| e.kind == PortMappingErrorKind::PersistenceFailed)
            {
                record.mapping.error = None;
                self.revision.fetch_add(1, Ordering::Release);
            }
        }
    }
    pub fn create(&self, request: PortMappingRequest) -> Result<PortMapping> {
        request.validate().map_err(|e| anyhow!(e))?;
        let _operation = self.operation.lock();
        let existing = self
            .mappings
            .lock()
            .iter()
            .find(|r| {
                let q = &r.mapping.request;
                q.kind == request.kind
                    && q.local_port == request.local_port
                    && q.remote_port == request.remote_port
                    && q.target_host == request.target_host
            })
            .map(|r| (r.mapping.id.clone(), r.mapping.state));
        if let Some((id, state)) = existing {
            let service_changed = {
                let mut records = self.mappings.lock();
                let record = records.iter_mut().find(|r| r.mapping.id == id).unwrap();
                let changed = record.mapping.request.service != request.service;
                record.mapping.request.service = request.service;
                if changed {
                    self.revision.fetch_add(1, Ordering::Release);
                }
                changed
            };
            if request.remember {
                self.remember_locked(&id, true)?;
            } else if service_changed {
                self.persist_mapping_state(&id);
            }
            if state == PortMappingState::Connected {
                return self.mapping(&id);
            }
            return self.start_locked(&id);
        }
        let id = SessionId::new_v4().to_string();
        self.mappings.lock().push(MappingRecord {
            mapping: PortMapping {
                id: id.clone(),
                local_port: request.local_port,
                remote_port: request.remote_port,
                request,
                state: PortMappingState::Connecting,
                error: None,
            },
            spec: None,
            enabled: true,
        });
        self.start_locked(&id)
    }
    pub fn mapping(&self, id: &str) -> Result<PortMapping> {
        self.mappings
            .lock()
            .iter()
            .find(|r| r.mapping.id == id)
            .map(|r| r.mapping.clone())
            .context("unknown port mapping")
    }
    pub fn start(&self, id: &str) -> Result<PortMapping> {
        let _operation = self.operation.lock();
        self.start_locked(id)
    }
    pub(super) fn start_locked(&self, id: &str) -> Result<PortMapping> {
        let request = {
            let mut records = self.mappings.lock();
            let record = records
                .iter_mut()
                .find(|r| r.mapping.id == id)
                .context("unknown port mapping")?;
            if record.mapping.state == PortMappingState::Connected {
                return Ok(record.mapping.clone());
            }
            record.enabled = true;
            record.mapping.state = PortMappingState::Connecting;
            self.revision.fetch_add(1, Ordering::Release);
            record.mapping.error = None;
            record.mapping.request.clone()
        };
        let connected = self
            .request("check", None)
            .is_ok_and(|o| o.status.success());
        self.update_connected(connected);
        let result = if !connected {
            Err(mapping_error(
                "SSH connection unavailable".into(),
                PortMappingErrorKind::ConnectionUnavailable,
            ))
        } else {
            self.open_mapping(&request)
        };
        {
            let mut records = self.mappings.lock();
            let record = records
                .iter_mut()
                .find(|r| r.mapping.id == id)
                .context("unknown port mapping")?;
            self.revision.fetch_add(1, Ordering::Release);
            match result {
                Ok((spec, local_port, remote_port)) => {
                    record.spec = Some(spec);
                    record.mapping.local_port = local_port;
                    record.mapping.remote_port = remote_port;
                    record.mapping.state = PortMappingState::Connected;
                }
                Err(error) => {
                    record.mapping.state =
                        if error.kind == PortMappingErrorKind::ConnectionUnavailable {
                            PortMappingState::Disconnected
                        } else {
                            PortMappingState::Failed
                        };
                    record.mapping.error = Some(error);
                }
            }
        }
        self.persist_mapping_state(id);
        self.mapping(id)
    }
    fn open_mapping(
        &self,
        request: &PortMappingRequest,
    ) -> Result<(PortForwardSpec, Option<u16>, Option<u16>), PortMappingError> {
        for attempt in 0..4 {
            let port = match request.kind {
                PortForwardKind::Local => choose_local_port(
                    request.local_port,
                    if attempt == 0 {
                        request.remote_port.unwrap()
                    } else {
                        0
                    },
                )?,
                PortForwardKind::Dynamic => {
                    choose_local_port(request.local_port, if attempt == 0 { 1080 } else { 0 })?
                }
                PortForwardKind::Remote => request.remote_port.unwrap_or(0),
            };
            let spec = PortForwardSpec {
                kind: request.kind,
                bind_address: "127.0.0.1".into(),
                port,
                destination: match request.kind {
                    PortForwardKind::Local => Some(target_address(
                        &request.target_host,
                        request.remote_port.unwrap(),
                    )),
                    PortForwardKind::Remote => Some(target_address(
                        &request.target_host,
                        request.local_port.unwrap(),
                    )),
                    PortForwardKind::Dynamic => None,
                },
            };
            let output = self
                .request("forward", Some(spec.tunnel_option()))
                .map_err(|e| mapping_error(e.to_string(), PortMappingErrorKind::RequestFailed))?;
            if output.status.success() {
                let (local, remote) = match request.kind {
                    PortForwardKind::Local => (Some(port), request.remote_port),
                    PortForwardKind::Dynamic => (Some(port), None),
                    PortForwardKind::Remote => {
                        let remote = if port == 0 {
                            String::from_utf8_lossy(&output.stdout)
                                .trim()
                                .parse::<u16>()
                                .ok()
                                .filter(|p| *p != 0)
                                .ok_or_else(|| {
                                    mapping_error(
                                        "SSH did not return the allocated port".into(),
                                        PortMappingErrorKind::RequestFailed,
                                    )
                                })?
                        } else {
                            port
                        };
                        (request.local_port, Some(remote))
                    }
                };
                return Ok((spec, local, remote));
            }
            let error = error_from_output(&output);
            // Availability probing and SSH binding are separate syscalls; retry an automatic port
            // only when another process wins that small race.
            if request.kind != PortForwardKind::Remote
                && request.local_port.is_none()
                && error.kind == PortMappingErrorKind::PortInUse
                && attempt < 3
            {
                continue;
            }
            return Err(error);
        }
        unreachable!()
    }
    pub fn stop(&self, id: &str, remove: bool) -> Result<PortMapping> {
        let _operation = self.operation.lock();
        let spec = {
            let mut records = self.mappings.lock();
            let record = records
                .iter_mut()
                .find(|r| r.mapping.id == id)
                .context("unknown port mapping")?;
            record.mapping.state = PortMappingState::Stopping;
            self.revision.fetch_add(1, Ordering::Release);
            record.spec.clone()
        };
        if let Some(spec) = spec
            && self
                .request("check", None)
                .is_ok_and(|o| o.status.success())
        {
            let result = self.request("cancel", Some(spec.tunnel_option()));
            let error = match result {
                Ok(output) if output.status.success() => None,
                Ok(output) => Some(error_from_output(&output)),
                Err(error) => Some(mapping_error(
                    error.to_string(),
                    PortMappingErrorKind::RequestFailed,
                )),
            };
            if let Some(error) = error {
                let mut records = self.mappings.lock();
                let record = records.iter_mut().find(|r| r.mapping.id == id).unwrap();
                record.mapping.state = PortMappingState::Connected;
                record.mapping.error = Some(error);
                self.revision.fetch_add(1, Ordering::Release);
                return Ok(record.mapping.clone());
            }
        }
        let mut mapping = {
            let mut records = self.mappings.lock();
            let record = records.iter_mut().find(|r| r.mapping.id == id).unwrap();
            record.enabled = false;
            record.spec = None;
            record.mapping.state = PortMappingState::Stopped;
            self.revision.fetch_add(1, Ordering::Release);
            record.mapping.error = None;
            let mapping = record.mapping.clone();
            if remove {
                records.retain(|r| r.mapping.id != id);
            }
            mapping
        };
        if let Err(error) = self.persist_mappings() {
            mapping.error = Some(mapping_error(
                error.to_string(),
                PortMappingErrorKind::PersistenceFailed,
            ));
            let mut records = self.mappings.lock();
            if let Some(record) = records.iter_mut().find(|r| r.mapping.id == id) {
                record.mapping = mapping.clone();
            } else {
                records.push(MappingRecord {
                    mapping: mapping.clone(),
                    spec: None,
                    enabled: false,
                });
            }
            self.revision.fetch_add(1, Ordering::Release);
        }
        Ok(mapping)
    }
    pub fn remember(&self, id: &str, remember: bool) -> Result<PortMapping> {
        let _operation = self.operation.lock();
        self.remember_locked(id, remember)
    }
    fn remember_locked(&self, id: &str, remember: bool) -> Result<PortMapping> {
        self.mappings
            .lock()
            .iter_mut()
            .find(|r| r.mapping.id == id)
            .context("unknown port mapping")?
            .mapping
            .request
            .remember = remember;
        self.revision.fetch_add(1, Ordering::Release);
        self.persist_mapping_state(id);
        self.mapping(id)
    }
}
