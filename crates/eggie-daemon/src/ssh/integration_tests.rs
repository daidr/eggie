//! Opt-in checks against an already authenticated master; every fixture uses a loopback listener.
use super::*;
use eggie_protocol::{PortForwardKind, PortMappingRequest, PortMappingState, PortServiceKind};
use std::net::{TcpListener, TcpStream};

struct Fixture {
    connection: SshConnection,
    mappings: Vec<String>,
    remote_pid: u32,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for id in &self.mappings {
            let _ = self.connection.stop(id, true);
        }
        if self.remote_pid != 0 {
            let _ = self
                .connection
                .run(&format!("kill {} 2>/dev/null || true", self.remote_pid));
        }
    }
}
fn stream(port: u16) -> TcpStream {
    let stream =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(5)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
}
fn http(mut stream: TcpStream) -> String {
    stream
        .write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}
fn request(kind: PortForwardKind, local: Option<u16>, remote: Option<u16>) -> PortMappingRequest {
    PortMappingRequest {
        kind,
        local_port: local,
        remote_port: remote,
        target_host: "127.0.0.1".into(),
        service: PortServiceKind::Http,
        remember: false,
    }
}

#[test]
#[ignore = "requires EGGIE_SSH_TEST_HOST and EGGIE_SSH_TEST_CONTROL_PATH for an authenticated master"]
fn real_ssh_local_reverse_socks_and_discovery() {
    let mut target = RemoteTarget::new(std::env::var("EGGIE_SSH_TEST_HOST").unwrap());
    target.user = std::env::var("EGGIE_SSH_TEST_USER").ok();
    target.port = std::env::var("EGGIE_SSH_TEST_PORT")
        .ok()
        .map(|s| s.parse().unwrap());
    let pool = SshConnections::default();
    let mut connection = SshConnection::new(target, pool.store.clone(), Vec::new()).unwrap();
    connection.path = std::env::var("EGGIE_SSH_TEST_CONTROL_PATH").unwrap().into();
    let mut fixture = Fixture {
        connection,
        mappings: Vec::new(),
        remote_pid: 0,
    };
    let marker = format!("eggie-ports-qa-{}", SessionId::new_v4().simple());
    let script = format!(
        r#"import os,socket,time,json
s=socket.socket();s.bind(("127.0.0.1",0));s.listen(8);s.settimeout(1)
port=s.getsockname()[1];pid=os.fork()
if pid:
 print(json.dumps({{"pid":pid,"port":port}}),flush=True);os._exit(0)
os.setsid()
f=os.open("/dev/null",os.O_RDWR)
for fd in (0,1,2):os.dup2(f,fd)
body=b"{marker}";deadline=time.monotonic()+120
while time.monotonic()<deadline:
 try:
  c,a=s.accept();c.settimeout(3);c.recv(4096);c.sendall(b"HTTP/1.0 200 OK\r\nContent-Length: "+str(len(body)).encode()+b"\r\n\r\n"+body);c.close()
 except (socket.timeout,OSError):pass
s.close()
"#
    );
    let quoted_script = format!("'{}'", script.replace('\'', "'\"'\"'"));
    let output = fixture
        .connection
        .run(&format!("python3 -c {quoted_script}"))
        .unwrap();
    let server: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    fixture.remote_pid = server["pid"].as_u64().unwrap() as u32;
    let remote = server["port"].as_u64().unwrap() as u16;

    let local = fixture
        .connection
        .create(request(PortForwardKind::Local, None, Some(remote)))
        .unwrap();
    fixture.mappings.push(local.id.clone());
    assert_eq!(local.state, PortMappingState::Connected, "{local:?}");
    assert!(http(stream(local.local_port.unwrap())).ends_with(&marker));
    fixture.connection.inspection_output().unwrap();
    assert!(
        fixture
            .connection
            .snapshot()
            .services
            .iter()
            .any(|s| s.port == remote)
    );
    assert_eq!(
        fixture.connection.stop(&local.id, false).unwrap().state,
        PortMappingState::Stopped
    );
    assert!(TcpStream::connect(("127.0.0.1", local.local_port.unwrap())).is_err());

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let local_port = listener.local_addr().unwrap().port();
    let reply = marker.clone();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok((mut stream, _)) = listener.accept() {
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut buffer = [0; 4096];
                stream.read(&mut buffer).unwrap();
                write!(
                    stream,
                    "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    reply.len(),
                    reply
                )
                .unwrap();
                break;
            }
            assert!(
                Instant::now() < deadline,
                "reverse connection did not arrive"
            );
            thread::sleep(Duration::from_millis(10));
        }
    });
    let reverse = fixture
        .connection
        .create(request(PortForwardKind::Remote, Some(local_port), None))
        .unwrap();
    fixture.mappings.push(reverse.id.clone());
    assert_eq!(reverse.state, PortMappingState::Connected, "{reverse:?}");
    let remote_port = reverse.remote_port.unwrap();
    let output = fixture.connection.run(&format!("python3 -c 'import urllib.request;print(urllib.request.urlopen(\"http://127.0.0.1:{remote_port}\",timeout=5).read().decode())'")).unwrap();
    assert_eq!(output.trim(), marker);
    server.join().unwrap();
    let stopped = fixture.connection.stop(&reverse.id, false).unwrap();
    assert_eq!(stopped.state, PortMappingState::Stopped, "{stopped:?}");
    let output = fixture.connection.run(&format!("python3 -c 'import socket;s=socket.socket();s.settimeout(2);print(s.connect_ex((\"127.0.0.1\",{remote_port})))'")).unwrap();
    assert_ne!(output.trim(), "0", "reverse listener was not closed");

    let socks = fixture
        .connection
        .create(request(PortForwardKind::Dynamic, None, None))
        .unwrap();
    fixture.mappings.push(socks.id.clone());
    assert_eq!(socks.state, PortMappingState::Connected, "{socks:?}");
    let mut proxy = stream(socks.local_port.unwrap());
    proxy.write_all(&[5, 1, 0]).unwrap();
    let mut greeting = [0; 2];
    proxy.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 0]);
    let mut connect = vec![5, 1, 0, 1, 127, 0, 0, 1];
    connect.extend_from_slice(&remote.to_be_bytes());
    proxy.write_all(&connect).unwrap();
    let mut response = [0; 10];
    proxy.read_exact(&mut response).unwrap();
    assert_eq!(&response[..2], &[5, 0]);
    assert!(http(proxy).ends_with(&marker));
    assert_eq!(
        fixture.connection.stop(&socks.id, false).unwrap().state,
        PortMappingState::Stopped
    );
    assert!(TcpStream::connect(("127.0.0.1", socks.local_port.unwrap())).is_err());
}
