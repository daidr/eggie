use super::*;

pub(super) fn visible_active_terminal_sessions(layout: &LayoutNode) -> Vec<SessionId> {
    fn collect(layout: &LayoutNode, sessions: &mut Vec<SessionId>) {
        match layout {
            LayoutNode::Group { group } => {
                if let Some(session_id) = group.active_item().and_then(|item| item.session_id) {
                    sessions.push(session_id);
                }
            }
            LayoutNode::Split { first, second, .. } => {
                collect(first, sessions);
                collect(second, sessions);
            }
        }
    }

    let mut sessions = Vec::new();
    collect(layout, &mut sessions);
    sessions
}

pub(super) fn layout_has_visible_active_terminal(
    layout: &LayoutNode,
    session_id: SessionId,
) -> bool {
    match layout {
        LayoutNode::Group { group } => group
            .active_item()
            .is_some_and(|item| item.session_id == Some(session_id)),
        LayoutNode::Split { first, second, .. } => {
            layout_has_visible_active_terminal(first, session_id)
                || layout_has_visible_active_terminal(second, session_id)
        }
    }
}

pub(super) fn terminal_container_corners(
    touches_left_layout_edge: bool,
    touches_right_layout_edge: bool,
    touches_bottom_layout_edge: bool,
    left_sidebar_collapsed: bool,
    right_sidebar_collapsed: bool,
) -> TerminalContainerCorners {
    // The tab bar always separates terminal content from the top window edge. A side corner is
    // rounded only on an outer layout edge that is separated from the window by an open sidebar;
    // corners meeting another group at a horizontal split stay square.
    let left_edge_allows_rounding = touches_left_layout_edge && !left_sidebar_collapsed;
    let right_edge_allows_rounding = touches_right_layout_edge && !right_sidebar_collapsed;
    TerminalContainerCorners {
        top_left: left_edge_allows_rounding,
        top_right: right_edge_allows_rounding,
        bottom_left: left_edge_allows_rounding && !touches_bottom_layout_edge,
        bottom_right: right_edge_allows_rounding && !touches_bottom_layout_edge,
    }
}

pub(super) fn terminal_container_borders(
    touches_left_layout_edge: bool,
    touches_right_layout_edge: bool,
    touches_bottom_layout_edge: bool,
    left_sidebar_collapsed: bool,
    right_sidebar_collapsed: bool,
) -> TerminalContainerBorders {
    TerminalContainerBorders {
        // The left-hand group owns each internal vertical seam, so the border is segmented by
        // terminal content regions and never crosses a nested group's tab bar.
        left: touches_left_layout_edge && !left_sidebar_collapsed,
        right: !touches_right_layout_edge || !right_sidebar_collapsed,
        // The upper group owns each internal horizontal seam.
        bottom: !touches_bottom_layout_edge,
    }
}

/// Pick the working directory for a new terminal: inherit the active session's live cwd when it
/// names an existing directory, otherwise fall back to the project root. The existence check
/// covers the most common stale-summary case — a directory deleted between poll cycles, which
/// would otherwise make the PTY spawn fail (other failures, e.g. permissions lost after the check,
/// still surface as spawn errors).
pub(super) fn resolve_new_terminal_cwd(active_cwd: Option<&Path>, project_root: &Path) -> PathBuf {
    match active_cwd {
        Some(dir) if dir.is_dir() => dir.to_path_buf(),
        _ => project_root.to_path_buf(),
    }
}

/// The launch fields a new terminal is spawned with, resolved from its profile.
///
/// Separated from the [`ClientRequest::CreateSession`] call so the precedence rules are unit-testable
/// without a daemon or a window.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ResolvedSpawnConfig {
    pub(super) cwd: PathBuf,
    pub(super) shell_program: String,
    pub(super) shell_args: Vec<String>,
    pub(super) shell_features: String,
    pub(super) env: std::collections::BTreeMap<String, String>,
    /// Set for a remote profile: the daemon opens an `ssh` session instead of a local shell.
    pub(super) remote: Option<eggie_domain::RemoteTarget>,
    /// The profile's `cwd` when it is a remote profile — a path on the far side, which is why it does
    /// not go into `cwd` (the local directory the `ssh` process itself starts in).
    pub(super) remote_cwd: Option<String>,
}

/// A key identifying *which* sessions a window owns, for the appearance reconcile's change gate.
///
/// A wrapping sum rather than a count: closing one tab and opening another between frames leaves the
/// count identical, which would skip the reconcile and leave the new terminal on the previous palette.
/// Order-independent on purpose — reordering tabs cannot change any session's profile, so it must not
/// invalidate the gate.
pub(super) fn session_set_key(session_ids: impl Iterator<Item = SessionId>) -> u128 {
    session_ids.fold(0u128, |key, id| key.wrapping_add(id.as_u128()))
}

/// Whether `key` is usable as an environment variable name.
///
/// Forwards to [`eggie_domain::is_valid_env_key`], which owns the rule so the profile editor, this
/// spawn filter, and the daemon's remote bootstrap cannot drift apart. Locally, a rejected key would
/// fail the PTY spawn opaquely (NUL) or create an entry no shell can reference (`=`); remotely it
/// would execute on the far host. Since profiles can be hand-edited on disk, this filter still runs
/// even though the editor validates at entry.
pub(super) fn is_valid_env_key(key: &str) -> bool {
    eggie_domain::is_valid_env_key(key)
}

/// Resolve a profile into the launch fields for a new terminal.
///
/// Precedence, field by field:
/// - **cwd**: the profile's explicit `cwd` (it names a fixed directory on purpose, so it beats
///   inheritance) → else the active terminal's live cwd → else the project root. A profile `cwd`
///   that no longer exists is ignored rather than fatal, matching [`resolve_new_terminal_cwd`].
///   `project_root` itself is assumed to exist — it comes from the project's own configuration, and a
///   root deleted since it was configured surfaces as a PTY spawn error.
/// - **shell program/args**: the profile's, verbatim. Empty means "let the daemon fall back to
///   `$SHELL`", which is the same sentinel the daemon already understands.
/// - **shell features**: assembled from the profile's own toggles, not the global settings — a remote
///   or container profile has no local `eggie` binary worth putting on `PATH`.
/// - **env**: the profile's map, minus unusable keys ([`is_valid_env_key`]). The daemon additionally
///   drops the variables Eggie owns, so integration cannot be tampered with (enforced by
///   `profile_env_is_injected_but_never_shadows_eggie_variables`).
///
/// A remote profile's `cwd` is a path on the remote host, so it must not be sent as the local PTY
/// working directory nor tested for local existence; the local side starts in the inherited/project
/// directory and the remote bootstrap `cd`s once connected (that bootstrap lands in a later step).
pub(super) fn resolve_spawn_config(
    profile: &Profile,
    active_cwd: Option<&Path>,
    project_root: &Path,
) -> ResolvedSpawnConfig {
    let cwd = match profile.cwd.as_deref() {
        // Remote: `profile.cwd` describes the far side. Resolve the local cwd as usual.
        Some(_) if profile.is_remote() => resolve_new_terminal_cwd(active_cwd, project_root),
        Some(dir) if dir.is_dir() => dir.to_path_buf(),
        // A configured-but-missing local directory falls back rather than failing the spawn.
        _ => resolve_new_terminal_cwd(active_cwd, project_root),
    };
    let mut env = std::collections::BTreeMap::new();
    for (key, value) in &profile.env {
        if is_valid_env_key(key) {
            env.insert(key.clone(), value.clone());
        } else {
            eprintln!(
                "profile {:?}: ignoring unusable environment variable name {key:?}",
                profile.name
            );
        }
    }
    ResolvedSpawnConfig {
        cwd,
        shell_program: profile.shell_program.clone(),
        shell_args: profile.shell_args.clone(),
        shell_features: crate::settings::ShellFeatures {
            path: profile.shell_integration_path,
        }
        .to_env_string(),
        env,
        // A target whose host/user could be mistaken for an ssh option is dropped, not defanged: the
        // profile is malformed, and connecting to a *different* place than the user wrote would be
        // worse than not connecting. Dropping it makes the profile launch a plain local shell, which
        // is visible and harmless.
        remote: profile
            .remote
            .as_ref()
            .filter(|target| {
                let usable = target.is_usable();
                if !usable {
                    eprintln!(
                        "profile {:?}: ignoring unusable remote target {:?}",
                        profile.name, target.host
                    );
                }
                usable
            })
            .cloned(),
        // Only meaningful for a remote profile; a local one's cwd already went into `cwd` above.
        remote_cwd: profile
            .is_remote()
            .then(|| {
                profile
                    .cwd
                    .as_ref()
                    .map(|cwd| cwd.to_string_lossy().into_owned())
            })
            .flatten(),
    }
}

pub(super) fn progress_label(progress: TerminalProgress, language: Language) -> String {
    let status = match (progress.state, progress.percent) {
        (TerminalProgressState::Normal, Some(100)) => language.progress_complete(),
        (TerminalProgressState::Normal, _) => language.progress_running(),
        (TerminalProgressState::Error, _) => language.progress_error(),
        (TerminalProgressState::Indeterminate, _) => language.progress_indeterminate(),
        (TerminalProgressState::Paused, _) => language.progress_paused(),
    };
    let percent = progress
        .percent
        .map(|percent| format!(" · {percent}%"))
        .unwrap_or_default();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let elapsed_seconds = now.saturating_sub(progress.updated_at_unix_ms) / 1_000;
    let recency = match elapsed_seconds {
        0..=1 => language.updated_just_now().to_owned(),
        2..=59 => language.updated_seconds_ago(elapsed_seconds),
        60..=3_599 => language.updated_minutes_ago(elapsed_seconds / 60),
        _ => language.updated_hours_ago(elapsed_seconds / 3_600),
    };
    format!("{status}{percent} · {recency}")
}

pub(super) fn paint_indicator_ring(
    bounds: Bounds<Pixels>,
    state: TerminalProgressState,
    percent: Option<u8>,
    phase: f32,
    color: u32,
    track_color: u32,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let radius = (width.min(height) / 2. - 1.5).max(1.);
    let center_x = f32::from(bounds.origin.x) + width / 2.;
    let center_y = f32::from(bounds.origin.y) + height / 2.;
    let top = point(px(center_x), px(center_y - radius));
    let bottom = point(px(center_x), px(center_y + radius));
    let radii = point(px(radius), px(radius));

    let mut track = PathBuilder::stroke(px(1.5));
    track.move_to(top);
    track.arc_to(radii, px(0.), false, true, bottom);
    track.arc_to(radii, px(0.), false, true, top);
    if let Ok(path) = track.build() {
        window.paint_path(path, rgb(track_color));
    }

    let (start_turn, fraction) = match state {
        TerminalProgressState::Indeterminate => (phase, 0.28),
        _ => (0., f32::from(percent.unwrap_or(100)) / 100.),
    };
    if fraction <= 0. {
        return;
    }

    let start_angle = -std::f32::consts::FRAC_PI_2 + start_turn * std::f32::consts::TAU;
    let start = point(
        px(center_x + radius * start_angle.cos()),
        px(center_y + radius * start_angle.sin()),
    );
    let mut arc = PathBuilder::stroke(px(1.5));
    arc.move_to(start);
    if fraction >= 0.999 {
        let opposite_angle = start_angle + std::f32::consts::PI;
        let opposite = point(
            px(center_x + radius * opposite_angle.cos()),
            px(center_y + radius * opposite_angle.sin()),
        );
        arc.arc_to(radii, px(0.), false, true, opposite);
        arc.arc_to(radii, px(0.), false, true, start);
    } else {
        let end_angle = start_angle + fraction * std::f32::consts::TAU;
        arc.arc_to(
            radii,
            px(0.),
            fraction > 0.5,
            true,
            point(
                px(center_x + radius * end_angle.cos()),
                px(center_y + radius * end_angle.sin()),
            ),
        );
    }
    if let Ok(path) = arc.build() {
        window.paint_path(path, rgb(color));
    }
}

pub(super) fn should_show_content_drop_target(
    source_group_id: GroupId,
    target_group_id: GroupId,
    source_item_count: usize,
) -> bool {
    source_group_id != target_group_id || source_item_count >= 2
}

pub(super) fn content_drop_zone(width: f32, height: f32, x: f32, y: f32) -> TabDropZone {
    let inside_center = x > width * CONTENT_SPLIT_EDGE_FACTOR
        && x < width * (1. - CONTENT_SPLIT_EDGE_FACTOR)
        && y > height * CONTENT_SPLIT_EDGE_FACTOR
        && y < height * (1. - CONTENT_SPLIT_EDGE_FACTOR);
    if inside_center {
        TabDropZone::Center
    } else if x < width / 3. {
        TabDropZone::Edge(Direction::Left)
    } else if x > width * 2. / 3. {
        TabDropZone::Edge(Direction::Right)
    } else if y < height / 2. {
        TabDropZone::Edge(Direction::Up)
    } else {
        TabDropZone::Edge(Direction::Down)
    }
}

pub(super) fn tab_bar_insertion_index(tab_index: usize, tab_width: f32, relative_x: f32) -> usize {
    if relative_x <= tab_width / 2. {
        tab_index
    } else {
        tab_index + 1
    }
}

#[cfg(test)]
pub(super) const KITTY_DISAMBIGUATE_ESCAPE_CODES: u8 = 1;
pub(super) const KITTY_REPORT_EVENT_TYPES: u8 = 2;
pub(super) const KITTY_REPORT_ALL_KEYS: u8 = 8;
pub(super) const KITTY_REPORT_ASSOCIATED_TEXT: u8 = 16;

pub(super) fn protocol_mouse_button(button: MouseButton) -> Option<TerminalMouseButton> {
    match button {
        MouseButton::Left => Some(TerminalMouseButton::Left),
        MouseButton::Middle => Some(TerminalMouseButton::Middle),
        MouseButton::Right => Some(TerminalMouseButton::Right),
        MouseButton::Navigate(_) => None,
    }
}

pub(super) fn protocol_terminal_modifiers(modifiers: gpui::Modifiers) -> TerminalModifiers {
    TerminalModifiers {
        shift: modifiers.shift,
        alt: modifiers.alt,
        control: modifiers.control,
    }
}

pub(super) fn notification_tag(session_id: SessionId, notification_id: &str) -> String {
    format!("eggie:{session_id}:{notification_id}")
}

pub(super) fn clipboard_item_from_contents(
    contents: Vec<TerminalClipboardContent>,
) -> Option<ClipboardItem> {
    let mut entries = Vec::new();
    let mut fallback_text = None;
    for content in contents {
        if content.mime_type.starts_with("text/plain") {
            if let Ok(text) = String::from_utf8(content.data) {
                entries.push(ClipboardEntry::String(ClipboardString::new(text)));
            }
        } else if content.mime_type.starts_with("text/") {
            if fallback_text.is_none() {
                fallback_text = String::from_utf8(content.data).ok();
            }
        } else if let Some(format) = ImageFormat::from_mime_type(&content.mime_type) {
            entries.push(ClipboardEntry::Image(Image::from_bytes(
                format,
                content.data,
            )));
        }
    }
    if !entries
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::String(_)))
        && let Some(text) = fallback_text
    {
        entries.insert(0, ClipboardEntry::String(ClipboardString::new(text)));
    }
    (!entries.is_empty()).then_some(ClipboardItem { entries })
}

pub(super) fn clipboard_contents_from_item(
    item: ClipboardItem,
    requested_mime_types: &[String],
) -> Vec<TerminalClipboardContent> {
    let accepts = |mime_type: &str| {
        let base = mime_type.split(';').next().unwrap_or(mime_type);
        requested_mime_types.is_empty()
            || requested_mime_types.iter().any(|requested| {
                let requested_base = requested.split(';').next().unwrap_or(requested);
                requested == "."
                    || requested == mime_type
                    || requested_base == base
                    || requested == "*/*"
                    || requested
                        .strip_suffix("/*")
                        .is_some_and(|prefix| mime_type.starts_with(prefix))
            })
    };
    let mut contents = Vec::new();
    for entry in item.into_entries() {
        match entry {
            ClipboardEntry::String(value) if accepts("text/plain") => {
                contents.push(TerminalClipboardContent {
                    mime_type: "text/plain".to_owned(),
                    data: value.into_text().into_bytes(),
                });
            }
            ClipboardEntry::Image(image) if accepts(image.format().mime_type()) => {
                contents.push(TerminalClipboardContent {
                    mime_type: image.format().mime_type().to_owned(),
                    data: image.bytes().to_vec(),
                });
            }
            ClipboardEntry::ExternalPaths(paths) if accepts("text/uri-list") => {
                let text = paths
                    .0
                    .into_iter()
                    .map(|path| format!("file://{}", path.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                contents.push(TerminalClipboardContent {
                    mime_type: "text/uri-list".to_owned(),
                    data: text.into_bytes(),
                });
            }
            _ => {}
        }
    }
    contents
}

pub(super) fn fixed_terminal_scroll_delta(delta: f32) -> i32 {
    if !delta.is_finite() {
        return 0;
    }
    (f64::from(delta) * f64::from(TERMINAL_SCROLL_DELTA_SCALE))
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// Thumb geometry for the terminal scrollbar, in pixels relative to the track's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScrollbarThumb {
    pub(super) top: f32,
    pub(super) height: f32,
}

/// Compute the scrollbar thumb's height and top offset.
///
/// `H` = history_size (scrollback lines), `R` = visible rows, `D` = display_offset (0 = live bottom,
/// H = scrolled to the oldest line). The thumb represents the R visible rows out of the (H + R) total
/// scrollable rows. Viewport-top content index is `H - D`, mapped onto pixel travel `[0, travel]`:
/// D = 0 → thumb flush bottom, D = H → thumb flush top. Returns `None` when there is no scrollback
/// (`H == 0`) or the track is too short to be meaningful.
pub(super) fn scrollbar_thumb_geometry(
    track_height: f32,
    history: u32,
    rows: u16,
    offset: u32,
) -> Option<ScrollbarThumb> {
    if history == 0 || track_height <= 0. {
        return None;
    }
    let h = history as f32;
    let r = (rows.max(1)) as f32;
    let frac = r / (h + r);
    let thumb_height = (track_height * frac)
        .max(TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT)
        .min(track_height);
    let travel = (track_height - thumb_height).max(0.);
    let d = (offset.min(history)) as f32;
    let top = ((h - d) / h) * travel;
    Some(ScrollbarThumb {
        top,
        height: thumb_height,
    })
}

/// Inverse of [`scrollbar_thumb_geometry`]: map a thumb-top pixel position back to a scroll offset D.
/// `thumb_top` is clamped to `[0, travel]` by the caller's grab math; here we invert the linear map.
/// Returns the current offset unchanged when there is no travel (thumb fills the track).
pub(super) fn scrollbar_offset_from_thumb_top(
    track_height: f32,
    history: u32,
    rows: u16,
    thumb_top: f32,
) -> u32 {
    if history == 0 || track_height <= 0. {
        return 0;
    }
    let h = history as f32;
    let r = (rows.max(1)) as f32;
    let frac = r / (h + r);
    let thumb_height = (track_height * frac)
        .max(TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT)
        .min(track_height);
    let travel = track_height - thumb_height;
    if travel <= 0. {
        return 0;
    }
    let clamped = thumb_top.clamp(0., travel);
    // top = ((H - D) / H) * travel  ⇒  D = H * (1 - top/travel)
    let d = (h * (1. - clamped / travel)).round();
    (d.max(0.) as u32).min(history)
}

pub(super) fn terminal_size_for_viewport(
    width: f32,
    height: f32,
    cell_width: f32,
    line_height: f32,
    scale_factor: f32,
) -> TerminalSize {
    fn cells_that_fit(extent: f32, cell_extent: f32, minimum: u16) -> u16 {
        if !extent.is_finite() || !cell_extent.is_finite() || cell_extent <= 0. {
            return minimum;
        }
        ((extent / cell_extent).next_up().floor() as u32).clamp(minimum as u32, u16::MAX as u32)
            as u16
    }

    TerminalSize {
        columns: cells_that_fit(width, cell_width, 2),
        rows: cells_that_fit(height, line_height, 1),
        cell_width: (cell_width * scale_factor.max(1.))
            .round()
            .clamp(1., u16::MAX as f32) as u16,
        cell_height: (line_height * scale_factor.max(1.))
            .round()
            .clamp(1., u16::MAX as f32) as u16,
    }
}

pub(super) fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(super) fn section_label(label: &'static str, colors: UiColors) -> impl IntoElement {
    div().text_xs().text_color(rgb(colors.muted)).child(label)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProcessTreeRow {
    pub(super) process_index: usize,
    pub(super) branch_has_next: Vec<bool>,
}

pub(super) fn kill_process(pid: u32, signal: i32) {
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

#[cfg(target_os = "macos")]
pub(super) fn executable_path_for_pid(pid: u32) -> Option<String> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let len = unsafe {
        libc::proc_pidpath(
            pid as libc::pid_t,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        )
    };
    if len <= 0 {
        return None;
    }
    buffer.truncate(len as usize);
    String::from_utf8(buffer).ok()
}

#[cfg(not(target_os = "macos"))]
pub(super) fn executable_path_for_pid(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()?
        .to_str()
        .map(str::to_owned)
}

pub(super) fn process_tree_rows(processes: &[ProcessInfo]) -> Vec<ProcessTreeRow> {
    fn sort_indices(indices: &mut [usize], processes: &[ProcessInfo]) {
        indices.sort_by(|left, right| {
            processes[*left]
                .pid
                .cmp(&processes[*right].pid)
                .then_with(|| processes[*left].name.cmp(&processes[*right].name))
        });
    }

    fn visit(
        process_index: usize,
        processes: &[ProcessInfo],
        children: &HashMap<u32, Vec<usize>>,
        branch_has_next: &mut Vec<bool>,
        visited: &mut HashSet<usize>,
        rows: &mut Vec<ProcessTreeRow>,
    ) {
        if !visited.insert(process_index) {
            return;
        }

        rows.push(ProcessTreeRow {
            process_index,
            branch_has_next: branch_has_next.clone(),
        });

        if let Some(child_indices) = children.get(&processes[process_index].pid) {
            for (position, child_index) in child_indices.iter().copied().enumerate() {
                branch_has_next.push(position + 1 < child_indices.len());
                visit(
                    child_index,
                    processes,
                    children,
                    branch_has_next,
                    visited,
                    rows,
                );
                branch_has_next.pop();
            }
        }
    }

    let pids = processes
        .iter()
        .map(|process| process.pid)
        .collect::<HashSet<_>>();
    let mut roots = Vec::new();
    let mut children = HashMap::<u32, Vec<usize>>::new();
    for (index, process) in processes.iter().enumerate() {
        match process.parent_pid {
            Some(parent_pid) if parent_pid != process.pid && pids.contains(&parent_pid) => {
                children.entry(parent_pid).or_default().push(index);
            }
            _ => roots.push(index),
        }
    }
    sort_indices(&mut roots, processes);
    for child_indices in children.values_mut() {
        sort_indices(child_indices, processes);
    }

    let mut rows = Vec::with_capacity(processes.len());
    let mut visited = HashSet::with_capacity(processes.len());
    for root in roots {
        visit(
            root,
            processes,
            &children,
            &mut Vec::new(),
            &mut visited,
            &mut rows,
        );
    }

    let mut malformed_roots = (0..processes.len())
        .filter(|index| !visited.contains(index))
        .collect::<Vec<_>>();
    sort_indices(&mut malformed_roots, processes);
    for root in malformed_roots {
        visit(
            root,
            processes,
            &children,
            &mut Vec::new(),
            &mut visited,
            &mut rows,
        );
    }
    rows
}

pub(super) fn process_section(
    processes: &[ProcessInfo],
    colors: UiColors,
    language: Language,
    is_remote: bool,
    cx: &mut Context<EggieApp>,
) -> AnyElement {
    let body = if processes.is_empty() {
        placeholder(language.no_running_processes(), colors).into_any_element()
    } else {
        let pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
        let root_pids: HashSet<u32> = processes
            .iter()
            .filter(|p| match p.parent_pid {
                Some(parent) => parent == p.pid || !pids.contains(&parent),
                None => true,
            })
            .map(|p| p.pid)
            .collect();

        let mut rows = div().flex().flex_col();
        for row in process_tree_rows(processes) {
            let process = &processes[row.process_index];
            let is_root = root_pids.contains(&process.pid);
            rows = rows.child(process_info_row(
                process,
                row.branch_has_next,
                colors,
                is_root,
                is_remote,
                cx,
            ));
        }
        rows.into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(process_section_header(colors, language))
        .child(body)
        .into_any_element()
}

pub(super) fn process_section_header(colors: UiColors, language: Language) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .text_size(px(10.))
        .text_color(rgb(colors.muted))
        .child(language.processes_label())
}

pub(super) fn ports_section(
    inspection: &SessionInspection,
    colors: UiColors,
    language: Language,
) -> Option<AnyElement> {
    if inspection.ports.is_empty() {
        return None;
    }

    let mut rows = div().flex().flex_col().gap_2();
    for port in &inspection.ports {
        let process_name = inspection
            .processes
            .iter()
            .find(|process| process.pid == port.pid)
            .map(|process| process.name.as_str());
        rows = rows.child(listening_port_row(port, process_name, colors));
    }
    Some(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_label(language.ports_label(), colors))
            .child(rows)
            .into_any_element(),
    )
}

pub(super) fn info_row(label: &'static str, value: String, colors: UiColors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap_1()
        .child(div().text_xs().text_color(rgb(colors.muted)).child(label))
        .child(div().min_w_0().truncate().child(value))
}

/// Parse a port forward spec from the prompt: `L 8080 localhost:80`, `R 8080 localhost:80`, or
/// `D 1080`. Returns None for a malformed spec.
pub(super) fn parse_forward_spec(text: &str) -> Option<eggie_protocol::PortForwardSpec> {
    let mut parts = text.split_whitespace();
    let kind = match parts.next()?.to_ascii_uppercase().as_str() {
        "L" => eggie_protocol::PortForwardKind::Local,
        "R" => eggie_protocol::PortForwardKind::Remote,
        "D" => eggie_protocol::PortForwardKind::Dynamic,
        _ => return None,
    };
    let port = parts.next()?.parse::<u16>().ok()?;
    let destination = parts.next().map(str::to_owned);
    if matches!(
        kind,
        eggie_protocol::PortForwardKind::Local | eggie_protocol::PortForwardKind::Remote
    ) && destination.is_none()
    {
        return None;
    }
    Some(eggie_protocol::PortForwardSpec {
        kind,
        bind_address: "127.0.0.1".to_owned(),
        port,
        destination,
    })
}

/// The directory to display for a session: the remote cwd when the session is remote, otherwise the
/// local cwd. A remote path is shown as `user@host:path` so it is not mistaken for a local path.
pub(super) fn displayed_directory(session: &eggie_protocol::SessionSummary) -> String {
    if let Some(location) = session.reported_location.as_ref()
        && !location.local
        && !location.path.is_empty()
    {
        let authority = match (location.user.as_deref(), location.host.as_deref()) {
            (Some(user), Some(host)) => format!("{user}@{host}"),
            (None, Some(host)) => host.to_owned(),
            _ => return location.path.clone(),
        };
        return format!("{authority}:{}", location.path);
    }
    session.current_directory.display().to_string()
}

pub(super) fn process_summary(
    process: &eggie_protocol::ProcessSummary,
    colors: UiColors,
    language: Language,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap_1()
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .child(process.name.clone()),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(colors.muted))
                .child(format!("{} {}", language.pid_label(), process.pid)),
        )
}

pub(super) fn process_info_row(
    process: &ProcessInfo,
    branch_has_next: Vec<bool>,
    colors: UiColors,
    is_root: bool,
    is_remote: bool,
    cx: &mut Context<EggieApp>,
) -> impl IntoElement {
    let pid = process.pid;
    let mut content = div()
        .id(format!("process-row-{pid}"))
        .flex()
        .flex_1()
        .items_center()
        .min_w_0()
        .h(px(PROCESS_ROW_HEIGHT))
        .rounded_md()
        .ml_neg_1()
        .mr_neg_2();

    // A remote process cannot be killed or inspected locally: kill would signal a local process with
    // the same pid, and the executable path is on the far side. So the context menu is omitted for a
    // remote session.
    if !is_root && !is_remote {
        content = content
            .cursor_pointer()
            .hover(|element| element.bg(rgb(colors.hover)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |app, _, window, cx| {
                    app.show_process_context_menu(pid, window, cx);
                }),
            );
    }

    content = content
        .child(
            div()
                .ml_1()
                .min_w_0()
                .truncate()
                .child(process.name.clone()),
        )
        .child(
            div()
                .ml_1()
                .flex_shrink_0()
                .text_size(px(10.))
                .text_color(rgb(colors.muted))
                .child(process.pid.to_string()),
        )
        .child(
            div()
                .ml_auto()
                .mr_2()
                .flex_shrink_0()
                .text_right()
                .text_size(px(10.))
                .text_color(rgb(colors.muted))
                .child(format_process_metrics(process)),
        );

    div()
        .flex()
        .items_center()
        .w_full()
        .h(px(PROCESS_ROW_HEIGHT))
        .text_size(px(12.))
        .child(process_tree_connector(branch_has_next, colors))
        .child(content)
}

pub(super) fn process_tree_connector(branch_has_next: Vec<bool>, colors: UiColors) -> AnyElement {
    let width = branch_has_next.len() as f32 * PROCESS_TREE_LEVEL_WIDTH;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin_x = f32::from(bounds.origin.x);
            let origin_y = f32::from(bounds.origin.y);
            let height = f32::from(bounds.size.height);
            let middle_y = origin_y + height / 2.;

            let mut builder = PathBuilder::stroke(px(1.));
            for (depth, has_next) in branch_has_next.iter().copied().enumerate() {
                let line_x =
                    origin_x + depth as f32 * PROCESS_TREE_LEVEL_WIDTH + PROCESS_TREE_LINE_X;
                let is_current = depth + 1 == branch_has_next.len();

                if !is_current {
                    if has_next {
                        builder.move_to(point(px(line_x), px(origin_y)));
                        builder.line_to(point(px(line_x), px(origin_y + height)));
                    }
                    continue;
                }

                if has_next {
                    builder.move_to(point(px(line_x), px(origin_y)));
                    builder.line_to(point(px(line_x), px(origin_y + height)));
                    builder.move_to(point(px(line_x), px(middle_y)));
                    builder.line_to(point(px(line_x + PROCESS_TREE_ARM_LENGTH), px(middle_y)));
                } else {
                    builder.move_to(point(px(line_x), px(origin_y)));
                    builder.line_to(point(px(line_x), px(middle_y - PROCESS_TREE_CORNER_RADIUS)));
                    builder.curve_to(
                        point(px(line_x + PROCESS_TREE_CORNER_RADIUS), px(middle_y)),
                        point(px(line_x), px(middle_y)),
                    );
                    builder.line_to(point(px(line_x + PROCESS_TREE_ARM_LENGTH), px(middle_y)));
                }
            }

            if let Ok(path) = builder.build() {
                window.paint_path(path, rgb(colors.muted));
            }
        },
    )
    .w(px(width))
    .h(px(PROCESS_ROW_HEIGHT))
    .flex_shrink_0()
    .into_any_element()
}

pub(super) fn cpu_usage_tenths_percent(usage: f32) -> u32 {
    if usage.is_finite() && usage > 0. {
        (usage * 10.).round().min(u32::MAX as f32) as u32
    } else {
        0
    }
}

pub(super) fn format_cpu_usage(usage_tenths_percent: Option<u32>) -> String {
    usage_tenths_percent
        .map(|usage| format!("{}.{:01}%", usage / 10, usage % 10))
        .unwrap_or_else(|| "—".to_owned())
}

pub(super) fn format_memory_usage(memory_bytes: Option<u64>) -> String {
    let Some(bytes) = memory_bytes else {
        return "—".to_owned();
    };
    const KIB: f64 = 1024.;
    const MIB: f64 = KIB * 1024.;
    const GIB: f64 = MIB * 1024.;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1}GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1}MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.0}KB", bytes / KIB)
    } else {
        format!("{}B", bytes as u64)
    }
}

pub(super) fn format_process_metrics(process: &ProcessInfo) -> String {
    format!(
        "{} / {}",
        format_cpu_usage(process.cpu_usage_tenths_percent),
        format_memory_usage(process.memory_bytes)
    )
}

pub(super) fn listening_port_row(
    port: &ListeningPort,
    process_name: Option<&str>,
    colors: UiColors,
) -> impl IntoElement {
    let owner = listening_port_owner(port.pid, process_name);
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(px(1.))
        .child(
            div()
                .min_w_0()
                .truncate()
                .child(format!("{} {}:{}", port.protocol, port.address, port.port)),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(10.))
                .text_color(rgb(colors.muted))
                .child(owner),
        )
}

pub(super) fn listening_port_owner(pid: u32, process_name: Option<&str>) -> String {
    process_name
        .map(|name| format!("{name}({pid})"))
        .unwrap_or_else(|| pid.to_string())
}

pub(super) fn placeholder(text: impl Into<SharedString>, colors: UiColors) -> impl IntoElement {
    div()
        .p_3()
        .rounded_md()
        .bg(rgb(colors.panel_alt))
        .text_color(rgb(colors.muted))
        .child(text.into())
}

pub(super) fn icon_button(
    icon_name: IconName,
    id: impl Into<gpui::ElementId>,
    colors: UiColors,
    tooltip: Option<SharedString>,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded_lg()
        .text_color(rgb(colors.muted))
        .cursor_pointer()
        .hover(move |element| {
            element
                .bg(rgb(colors.panel_alt))
                .text_color(rgb(colors.text))
        })
        .when_some(tooltip, |element, text| {
            element.tooltip(tooltip_text(text, colors))
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(listener)
        .child(icon(icon_name))
}

/// Small button used inside the update popover.
pub(super) fn popover_button(
    label: &str,
    primary: bool,
    colors: UiColors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(format!("update-popover-{label}"))
        .flex_none()
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .rounded_lg()
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .when(primary, |this| {
            this.bg(rgb(colors.accent))
                .text_color(rgb(colors.accent_foreground))
                .hover(move |element| element.bg(rgb(colors.accent_hover)))
        })
        .when(!primary, |this| {
            this.bg(rgb(colors.panel_alt))
                .text_color(rgb(colors.text))
                .border_1()
                .border_color(rgb(colors.border))
                .hover(move |element| element.bg(rgb(colors.hover)))
        })
        .on_click(on_click)
        .child(label.to_string())
        .into_any_element()
}

pub(super) fn format_update_speed(bytes_per_sec: u64) -> String {
    if bytes_per_sec >= 1_000_000 {
        format!("{:.1} MB/s", bytes_per_sec as f64 / 1_000_000.)
    } else if bytes_per_sec >= 1_000 {
        format!("{:.0} KB/s", bytes_per_sec as f64 / 1_000.)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}

pub(super) fn top_bar_icon_button(
    icon_name: IconName,
    id: impl Into<gpui::ElementId>,
    colors: UiColors,
    border_left: bool,
    border_right: bool,
    tooltip: Option<SharedString>,
    listener: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(TOP_BAR_ACTION_SLOT_WIDTH))
        .h_full()
        .when(border_left, |element| {
            element.border_l_1().border_color(rgb(colors.border))
        })
        .when(border_right, |element| {
            element.border_r_1().border_color(rgb(colors.border))
        })
        .child(icon_button(icon_name, id, colors, tooltip, listener))
}

/// The button glyph for a directory opener. A future custom opener falls back to
/// [`IconName::AppWindow`].
pub(super) fn icon_for_open_with(opener: OpenWith) -> IconName {
    match opener {
        OpenWith::Finder => IconName::Finder,
        OpenWith::VsCode => IconName::VsCode,
    }
}

pub(super) fn open_with_label(language: Language, opener: OpenWith) -> &'static str {
    match opener {
        OpenWith::Finder => language.open_with_finder(),
        OpenWith::VsCode => language.open_with_vscode(),
    }
}

/// Open `path` in the chosen application via macOS `open`. Finder opens the directory directly;
/// VS Code is launched by bundle name so it works without the `code` CLI on `PATH`. Failures are
/// logged, not surfaced — the button is best-effort.
pub(super) fn open_path_with(path: &Path, opener: OpenWith) {
    let mut command = std::process::Command::new("/usr/bin/open");
    match opener {
        OpenWith::Finder => {
            command.arg(path);
        }
        OpenWith::VsCode => {
            command.args(["-a", "Visual Studio Code"]).arg(path);
        }
    }
    if let Err(error) = command.spawn() {
        eprintln!(
            "failed to open {} with {}: {error}",
            path.display(),
            opener.slug()
        );
    }
}
