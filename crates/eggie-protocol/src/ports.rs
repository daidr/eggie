use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortServiceKind {
    #[default]
    Auto,
    Http,
    Https,
    Tcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMappingRequest {
    pub kind: PortForwardKind,
    /// None chooses an available local port. Required as the target for a reverse mapping.
    pub local_port: Option<u16>,
    /// Required as the target for a local mapping. None lets SSH allocate a reverse listener.
    pub remote_port: Option<u16>,
    pub target_host: String,
    pub service: PortServiceKind,
    pub remember: bool,
}

impl PortMappingRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.local_port == Some(0) || self.remote_port == Some(0) {
            return Err("invalid_port");
        }
        if self.kind == PortForwardKind::Local && self.remote_port.is_none()
            || self.kind == PortForwardKind::Remote && self.local_port.is_none()
        {
            return Err("missing_target_port");
        }
        if self.kind != PortForwardKind::Dynamic
            && (self.target_host.is_empty()
                || self.target_host.starts_with('-')
                || self.target_host.chars().any(|c| {
                    c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '[' | ']')
                })
                || self.target_host.contains(':')
                    && self.target_host.parse::<std::net::Ipv6Addr>().is_err())
        {
            return Err("invalid_target_host");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortMappingState {
    Connecting,
    Connected,
    Stopping,
    Stopped,
    Disconnected,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortMappingErrorKind {
    PortInUse,
    ConnectionUnavailable,
    AccessDenied,
    RequestFailed,
    PersistenceFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMappingError {
    pub kind: PortMappingErrorKind,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMapping {
    pub id: String,
    pub request: PortMappingRequest,
    pub local_port: Option<u16>,
    pub remote_port: Option<u16>,
    pub state: PortMappingState,
    pub error: Option<PortMappingError>,
}

impl PortMapping {
    pub fn local_address(&self) -> Option<String> {
        self.local_port.map(|port| format!("127.0.0.1:{port}"))
    }

    pub fn browser_url(&self) -> Option<String> {
        if self.state != PortMappingState::Connected || self.request.kind != PortForwardKind::Local
        {
            return None;
        }
        let scheme = match self.request.service {
            PortServiceKind::Http => "http",
            PortServiceKind::Https => "https",
            PortServiceKind::Auto if matches!(self.request.remote_port, Some(443 | 8443)) => {
                "https"
            }
            PortServiceKind::Auto
                if matches!(
                    self.request.remote_port,
                    Some(80 | 3000 | 3001 | 4200 | 5000 | 5173 | 8000 | 8080 | 8081 | 8888 | 9000)
                ) =>
            {
                "http"
            }
            _ => return None,
        };
        self.local_address()
            .map(|address| format!("{scheme}://{address}"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortMappingsSnapshot {
    pub revision: u64,
    pub registry_revision: u64,
    pub connection_id: String,
    pub destination: String,
    pub target: eggie_domain::RemoteTarget,
    pub connected: bool,
    pub mappings: Vec<PortMapping>,
    pub services: Vec<RemoteService>,
    pub discovery_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteService {
    pub port: u16,
    pub address: String,
    pub process: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> PortMappingRequest {
        PortMappingRequest {
            kind: PortForwardKind::Local,
            local_port: None,
            remote_port: Some(3000),
            target_host: "127.0.0.1".into(),
            service: PortServiceKind::Auto,
            remember: false,
        }
    }
    #[test]
    fn validates_ports_and_literal_hosts_without_command_syntax() {
        let mut request = request();
        assert!(request.validate().is_ok());
        for host in ["localhost", "::1", "example.internal"] {
            request.target_host = host.into();
            assert!(request.validate().is_ok());
        }
        for host in [
            "",
            "-oProxyCommand=test",
            "a b",
            "[::1]",
            "a:22",
            "x\n",
            "a/b",
        ] {
            request.target_host = host.into();
            assert!(request.validate().is_err());
        }
        request.target_host = "localhost".into();
        request.remote_port = Some(0);
        assert!(request.validate().is_err());
        request.remote_port = None;
        assert!(request.validate().is_err());
        request.kind = PortForwardKind::Remote;
        request.local_port = Some(1234);
        assert!(request.validate().is_ok());
        request.kind = PortForwardKind::Dynamic;
        request.local_port = None;
        assert!(request.validate().is_ok());
    }
    #[test]
    fn only_web_services_offer_a_browser_action() {
        let mut mapping = PortMapping {
            id: "test".into(),
            request: request(),
            local_port: Some(49152),
            remote_port: Some(3000),
            state: PortMappingState::Connected,
            error: None,
        };
        assert_eq!(
            mapping.browser_url().as_deref(),
            Some("http://127.0.0.1:49152")
        );
        mapping.request.remote_port = Some(5432);
        assert!(mapping.browser_url().is_none());
        mapping.request.service = PortServiceKind::Https;
        assert_eq!(
            mapping.browser_url().as_deref(),
            Some("https://127.0.0.1:49152")
        );
        mapping.request.kind = PortForwardKind::Remote;
        assert!(mapping.browser_url().is_none());
        mapping.request.kind = PortForwardKind::Local;
        mapping.state = PortMappingState::Disconnected;
        assert!(mapping.browser_url().is_none());
    }
}
