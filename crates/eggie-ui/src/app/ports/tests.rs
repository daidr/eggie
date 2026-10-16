use super::*;

#[gpui::test]
fn service_hover_redraws_on_enter_and_leave_without_other_updates(cx: &mut gpui::TestAppContext) {
    use std::{cell::Cell, rc::Rc};

    struct ServiceView(Rc<Cell<usize>>);
    impl Render for ServiceView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.0.set(self.0.get() + 1);
            let colors = UiColors {
                background: 0,
                panel: 0,
                panel_alt: 0,
                hover: 0x334455,
                border: 0,
                text: 0,
                muted: 0,
                accent: 0,
                accent_foreground: 0,
                accent_hover: 0,
            };
            div().size_full().p(px(20.)).child(
                render::service_row(
                    "connection",
                    &eggie_protocol::RemoteService {
                        address: "127.0.0.1".into(),
                        port: 3000,
                        process: "node".into(),
                    },
                    colors,
                )
                .w(px(200.))
                .h(px(40.)),
            )
        }
    }

    let renders = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, _| ServiceView(renders.clone()));
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let initial = renders.get();
    let move_mouse = |cx: &mut gpui::TestAppContext, x, y| {
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_mouse_move(point(px(x), px(y)), cx);
        })
        .unwrap();
    };

    move_mouse(cx, 30., 30.);
    assert_eq!(renders.get(), initial + 1, "enter must request a repaint");
    move_mouse(cx, 40., 30.);
    assert_eq!(renders.get(), initial + 1, "moving within a row needs no repaint");
    move_mouse(cx, 5., 5.);
    assert_eq!(renders.get(), initial + 2, "leave must clear the highlight");
}

#[test]
fn port_mappings_follow_the_current_ssh_session_without_a_global_fallback() {
    let local = SessionId::new_v4();
    let remote_a = SessionId::new_v4();
    let remote_b = SessionId::new_v4();
    let connection = |id: &str, revision| PortMappingsSnapshot {
        connection_id: id.into(),
        revision,
        registry_revision: 1,
        destination: id.into(),
        target: eggie_domain::RemoteTarget::new(id.into()),
        connected: true,
        mappings: Vec::new(),
        services: Vec::new(),
        discovery_error: None,
    };
    let inspections = [
        (local, None),
        (remote_a, Some(connection("a", 1))),
        (remote_b, Some(connection("b", 1))),
    ]
    .into_iter()
    .map(|(session_id, port_mappings)| {
        (
            session_id,
            SessionInspection {
                session_id,
                port_mappings,
                processes: Vec::new(),
                ports: Vec::new(),
            },
        )
    })
    .collect::<HashMap<_, _>>();
    let connections = vec![connection("b", 3), connection("a", 2)];
    assert!(port_snapshot_for_session(local, &inspections, &connections).is_none());
    assert!(port_snapshot_for_session(SessionId::new_v4(), &inspections, &connections).is_none());
    let a = port_snapshot_for_session(remote_a, &inspections, &connections).unwrap();
    assert_eq!(a.connection_id, "a");
    assert_eq!(a.revision, 2);
    assert_eq!(
        port_snapshot_for_session(remote_b, &inspections, &connections)
            .unwrap()
            .connection_id,
        "b"
    );
}
#[test]
fn discovered_services_keep_their_specific_bind_address() {
    assert_eq!(service_target_host("0.0.0.0"), "127.0.0.1");
    assert_eq!(service_target_host("*"), "127.0.0.1");
    assert_eq!(service_target_host("::"), "::1");
    for address in ["127.0.0.1", "10.0.0.2", "::1", "2001:db8::1"] {
        assert_eq!(service_target_host(address), address);
    }
}
#[test]
fn port_fields_allow_auto_only_for_the_listener() {
    assert_eq!(parse_port("", false), Ok(None));
    assert!(parse_port("", true).is_err());
    assert_eq!(parse_port(" 3000 ", true), Ok(Some(3000)));
    for value in ["0", "65536", "-1", "1.0", "L 3000 localhost:80"] {
        assert!(parse_port(value, false).is_err());
    }
    assert_eq!(parse_port("65535", true), Ok(Some(65535)));
}
#[test]
fn errors_are_localized_and_have_no_sentence_punctuation() {
    for kind in [
        PortMappingErrorKind::PortInUse,
        PortMappingErrorKind::ConnectionUnavailable,
        PortMappingErrorKind::AccessDenied,
        PortMappingErrorKind::PersistenceFailed,
        PortMappingErrorKind::RequestFailed,
    ] {
        let label = error_label(kind, Language::SimplifiedChinese);
        assert!(!label.ends_with(['。', '.']));
        assert!(!label.is_empty());
    }
}
