use eggie_protocol::{ProcessSummary, SessionSummary};
use gpui::{Image, ImageFormat};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};
use tiny_skia::{BlendMode, FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};

/// Terminal icons follow the session's foreground process, never the editable tab title.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ProcessIcon {
    /// Blank card for a project with no tabs; never used as a terminal process icon.
    ProjectPlaceholder,
    Terminal,
    NodeJs,
    Python,
    Git,
    GitDiff,
    Go,
    Rust,
    BunJs,
    Zig,
    Codex,
    ClaudeCode,
    OpenCode,
    PiAgent,
    File,
    Browser,
    Settings,
}

impl ProcessIcon {
    fn source(self) -> &'static str {
        match self {
            Self::ProjectPlaceholder => {
                include_str!("../assets/process-icons/project-placeholder.svg")
            }
            Self::Terminal => include_str!("../assets/process-icons/terminal-mark.svg"),
            Self::NodeJs => include_str!("../assets/process-icons/nodejs.svg"),
            Self::Python => include_str!("../assets/process-icons/python.svg"),
            Self::Git => include_str!("../assets/process-icons/git.svg"),
            Self::GitDiff => include_str!("../assets/process-icons/git-diff.svg"),
            Self::Go => include_str!("../assets/process-icons/go.svg"),
            Self::Rust => include_str!("../assets/process-icons/rust.svg"),
            Self::BunJs => include_str!("../assets/process-icons/bunjs.svg"),
            Self::Zig => include_str!("../assets/process-icons/zig.svg"),
            Self::Codex => include_str!("../assets/process-icons/codex.svg"),
            Self::ClaudeCode => include_str!("../assets/process-icons/claude-code.svg"),
            Self::OpenCode => include_str!("../assets/process-icons/opencode.svg"),
            Self::PiAgent => include_str!("../assets/process-icons/pi-agent.svg"),
            Self::File => include_str!("../assets/process-icons/file.svg"),
            Self::Browser => include_str!("../assets/process-icons/browser.svg"),
            Self::Settings => include_str!("../assets/process-icons/settings.svg"),
        }
    }

    fn foreground(self, dark: bool, in_card: bool) -> u32 {
        match self {
            Self::ProjectPlaceholder => 0xffffff,
            Self::Terminal if dark && !in_card => 0xe9ecf2,
            Self::Terminal => 0x000000,
            Self::GitDiff | Self::ClaudeCode | Self::File | Self::Browser | Self::Settings => {
                0xffffff
            }
            Self::NodeJs => 0x3c873a,
            Self::Python => 0x3776ab,
            Self::Git => 0xf05032,
            Self::Go => 0x00add8,
            Self::Rust => 0xce422b,
            Self::BunJs => 0xbd6b4c,
            Self::Zig => 0xf7a41d,
            Self::Codex => 0x3941ff,
            Self::PiAgent => 0x4d9abf,
            Self::OpenCode if in_card => 0x202833,
            Self::OpenCode if dark => 0xe9ecf2,
            Self::OpenCode => 0x202833,
        }
    }

    // Match visual size across padded marks and dense, full-viewport artwork.
    fn glyph_scale(self) -> f32 {
        match self {
            Self::Codex => 0.94,
            Self::PiAgent => 0.66,
            _ => 0.78,
        }
    }

    fn background(self, dark: bool) -> u32 {
        match self {
            Self::ProjectPlaceholder if dark => 0x46504e,
            Self::ProjectPlaceholder => 0xece8dd,
            Self::Terminal => 0x253030,
            Self::NodeJs => 0xd8edcf,
            Self::Python => 0xe4effb,
            Self::Git => 0xffe3d8,
            Self::GitDiff => 0xffdfd7,
            Self::Go => 0xdaf4f6,
            Self::Rust => 0xf6e3db,
            Self::BunJs => 0xf8f1e0,
            Self::Zig => 0xffedc7,
            Self::Codex => 0xeeeef8,
            Self::ClaudeCode => 0xd97757,
            Self::OpenCode => 0xe5ebec,
            Self::PiAgent => 0xf4f0e8,
            Self::File => 0xe6effb,
            Self::Browser => 0xe1f4f5,
            Self::Settings => 0xf0eafb,
        }
    }
}

fn basename(name: &str) -> &str {
    Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(name)
}

pub(crate) fn for_process(process: &ProcessSummary) -> ProcessIcon {
    let name = basename(&process.name)
        .trim_start_matches('-')
        .to_ascii_lowercase();
    let argv = process.command_line.as_deref().unwrap_or(&[]);
    let agent = argv
        .iter()
        .skip(1)
        .map(|arg| arg.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let has_agent = |needle: &str| {
        agent.iter().any(|arg| {
            let base = basename(arg);
            base == needle
                || arg.contains(&format!("/{needle}/"))
                || arg.contains(&format!("/{needle}."))
        })
    };
    if name == "codex" || has_agent("codex") && matches!(name.as_str(), "node" | "nodejs" | "bun") {
        return ProcessIcon::Codex;
    }
    if name == "claude"
        || name == "claude-code"
        || has_agent("claude-code") && matches!(name.as_str(), "node" | "nodejs" | "bun")
    {
        return ProcessIcon::ClaudeCode;
    }
    if name == "opencode"
        || has_agent("opencode") && matches!(name.as_str(), "node" | "nodejs" | "bun")
    {
        return ProcessIcon::OpenCode;
    }
    if matches!(name.as_str(), "pi" | "pi-agent" | "pi-coding-agent")
        || matches!(name.as_str(), "node" | "nodejs" | "bun")
            && (has_agent("pi-coding-agent")
                || agent.first().is_some_and(|arg| basename(arg) == "pi"))
    {
        return ProcessIcon::PiAgent;
    }
    match name.as_str() {
        "node" | "nodejs" | "npm" | "npx" | "pnpm" | "yarn" => ProcessIcon::NodeJs,
        "bun" | "bunx" => ProcessIcon::BunJs,
        "git"
            if argv
                .iter()
                .skip(1)
                .take_while(|arg| *arg != "--")
                .any(|arg| arg == "diff") =>
        {
            ProcessIcon::GitDiff
        }
        "git" => ProcessIcon::Git,
        "go" | "gofmt" | "gopls" => ProcessIcon::Go,
        "cargo" | "rustc" | "rustup" | "rust-analyzer" => ProcessIcon::Rust,
        "zig" => ProcessIcon::Zig,
        name if name.starts_with("python") || name.starts_with("pypy") => ProcessIcon::Python,
        _ => ProcessIcon::Terminal,
    }
}

pub(crate) fn for_session(session: &SessionSummary) -> ProcessIcon {
    if let Some(app) = session
        .program_status
        .as_ref()
        .and_then(|status| status.app.as_deref())
    {
        let icon = for_process(&ProcessSummary {
            pid: 0,
            name: app.to_owned(),
            command_line: None,
        });
        if icon != ProcessIcon::Terminal {
            return icon;
        }
    }
    let detected = for_process(&session.current_process);
    if detected != ProcessIcon::Terminal && detected != ProcessIcon::Git {
        return detected;
    }
    let Some(command) = session.active_command.as_deref() else {
        return detected;
    };
    let args: Vec<String> = command.split_whitespace().map(str::to_owned).collect();
    let Some(name) = args.first() else {
        return detected;
    };
    let reported = for_process(&ProcessSummary {
        pid: 0,
        name: name.clone(),
        command_line: Some(args),
    });
    if reported == ProcessIcon::Terminal {
        detected
    } else {
        reported
    }
}

fn svg_body(icon: ProcessIcon) -> &'static str {
    svg_inner(icon.source())
}

fn svg_inner(source: &'static str) -> &'static str {
    let start = source.find('>').expect("embedded SVG root") + 1;
    let end = source.rfind("</svg>").expect("embedded SVG close");
    &source[start..end]
}

fn hex(color: u32) -> String {
    format!("#{color:06X}")
}

fn svg_image(markup: String) -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Svg, markup.into_bytes()))
}

pub(crate) fn stack_width(count: usize, card_size: f32) -> f32 {
    if count == 0 {
        0.
    } else {
        card_size * 1.28 + 4. + (count.min(3) - 1) as f32 * 5.
    }
}

pub(crate) fn stack_angle(index: usize, count: usize) -> f32 {
    if count == 1 {
        0.
    } else {
        match index {
            0 => -7.,
            1 if count == 2 => 7.,
            1 => 2.,
            _ => 9.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CardPose {
    pub icon: ProcessIcon,
    pub offset_x: f32,
    pub angle: f32,
    pub opacity: f32,
}

pub(crate) struct CardLayer {
    pub poses: Vec<CardPose>,
    pub weight: f32,
}

/// One card for a sidebar tab, or up to three cards for a project's icon group.
/// Angle is strictly a property of a stack: a single card is always upright.
pub(crate) fn card_stack(icons: &[ProcessIcon], dark: bool, card_size: f32) -> Option<Arc<Image>> {
    if icons.is_empty() {
        return None;
    }
    let icons = &icons[..icons.len().min(3)];
    let key = (icons.to_vec(), dark, card_size.to_bits());
    static CACHE: OnceLock<Mutex<HashMap<(Vec<ProcessIcon>, bool, u32), Arc<Image>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("process card cache poisoned");
    Some(
        cache
            .entry(key)
            .or_insert_with(|| {
                let poses = (0..icons.len())
                    .rev()
                    .map(|index| CardPose {
                        icon: icons[index],
                        offset_x: index as f32 * 5.,
                        angle: stack_angle(index, icons.len()),
                        opacity: 1.,
                    })
                    .collect::<Vec<_>>();
                svg_image(card_markup(
                    &poses,
                    dark,
                    card_size,
                    stack_width(icons.len(), card_size),
                ))
            })
            .clone(),
    )
}

/// Compose cached card artwork as the cards move; this keeps SVG parsing off the animation path.
pub(crate) fn card_frame(
    poses: &[CardPose],
    dark: bool,
    card_size: f32,
    width: f32,
) -> Arc<gpui::RenderImage> {
    card_render_image(card_pixmap(poses, dark, card_size, width))
}

/// Mixing complete paint orders keeps occlusion continuous when the front card changes.
pub(crate) fn card_layers_frame(
    layers: &[CardLayer],
    dark: bool,
    card_size: f32,
    width: f32,
) -> Arc<gpui::RenderImage> {
    let pixel_width = (width * 2.).ceil().max(1.) as u32;
    let pixel_height = ((card_size + 5.) * 2.).ceil() as u32;
    let mut frame = Pixmap::new(pixel_width, pixel_height).expect("process card canvas");
    for layer in layers {
        if layer.weight <= 0. {
            continue;
        }
        let image = card_pixmap(&layer.poses, dark, card_size, width);
        frame.draw_pixmap(
            0,
            0,
            image.as_ref(),
            &PixmapPaint {
                opacity: layer.weight,
                blend_mode: BlendMode::Plus,
                ..PixmapPaint::default()
            },
            Transform::identity(),
            None,
        );
    }
    card_render_image(frame)
}

fn card_pixmap(poses: &[CardPose], dark: bool, card_size: f32, width: f32) -> Pixmap {
    let pixel_width = (width * 2.).ceil().max(1.) as u32;
    let pixel_height = ((card_size + 5.) * 2.).ceil() as u32;
    let mut frame = Pixmap::new(pixel_width, pixel_height).expect("process card canvas");
    let center_x = (2. + card_size * 1.28 / 2.) * 2.;
    let center_y = (2. + card_size / 2.) * 2.;
    for pose in poses {
        if pose.opacity <= 0. {
            continue;
        }
        let tile = card_tile(pose.icon, dark, card_size);
        let transform = Transform::from_translate(pose.offset_x * 2., 0.)
            .pre_rotate_at(pose.angle, center_x, center_y);
        frame.draw_pixmap(
            0,
            0,
            tile.as_ref().as_ref(),
            &PixmapPaint {
                opacity: pose.opacity,
                quality: FilterQuality::Bilinear,
                ..PixmapPaint::default()
            },
            transform,
            None,
        );
    }
    frame
}

fn card_render_image(frame: Pixmap) -> Arc<gpui::RenderImage> {
    let pixel_width = frame.width();
    let pixel_height = frame.height();
    let mut pixels = frame.take();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer = image::ImageBuffer::from_raw(pixel_width, pixel_height, pixels)
        .expect("process card pixels");
    Arc::new(gpui::RenderImage::new(vec![image::Frame::new(buffer)]))
}

pub(crate) fn card_stack_frame(
    icons: &[ProcessIcon],
    dark: bool,
    card_size: f32,
) -> Option<Arc<gpui::RenderImage>> {
    if icons.is_empty() {
        return None;
    }
    let icons = &icons[..icons.len().min(3)];
    let key = (icons.to_vec(), dark, card_size.to_bits());
    static CACHE: OnceLock<Mutex<HashMap<(Vec<ProcessIcon>, bool, u32), Arc<gpui::RenderImage>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("process stack frame cache poisoned");
    Some(
        cache
            .entry(key)
            .or_insert_with(|| {
                let poses = (0..icons.len())
                    .rev()
                    .map(|index| CardPose {
                        icon: icons[index],
                        offset_x: index as f32 * 5.,
                        angle: stack_angle(index, icons.len()),
                        opacity: 1.,
                    })
                    .collect::<Vec<_>>();
                card_frame(&poses, dark, card_size, stack_width(icons.len(), card_size))
            })
            .clone(),
    )
}

fn card_tile(icon: ProcessIcon, dark: bool, card_size: f32) -> Arc<Pixmap> {
    let key = (icon, dark, card_size.to_bits());
    static CACHE: OnceLock<Mutex<HashMap<(ProcessIcon, bool, u32), Arc<Pixmap>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("process card tile cache poisoned");
    cache
        .entry(key)
        .or_insert_with(|| {
            static RENDERER: OnceLock<gpui::SvgRenderer> = OnceLock::new();
            let renderer = RENDERER.get_or_init(|| gpui::SvgRenderer::new(Arc::new(())));
            let markup = card_markup(
                &[CardPose {
                    icon,
                    offset_x: 0.,
                    angle: 0.,
                    opacity: 1.,
                }],
                dark,
                card_size,
                stack_width(1, card_size),
            );
            let image = renderer
                .render_single_frame(markup.as_bytes(), 1.)
                .expect("embedded process card renders");
            let size = image.size(0);
            let mut pixels = image.as_bytes(0).expect("process card pixels").to_vec();
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Arc::new(
                Pixmap::from_vec(
                    pixels,
                    IntSize::from_wh(size.width.0 as u32, size.height.0 as u32)
                        .expect("process card dimensions"),
                )
                .expect("process card pixmap"),
            )
        })
        .clone()
}

fn card_markup(poses: &[CardPose], dark: bool, card_size: f32, width: f32) -> String {
    let height = card_size + 5.;
    let card_width = card_size * 1.28;
    let mut body = String::new();
    for pose in poses {
        let icon = pose.icon;
        let x = 2. + pose.offset_x;
        let y = 2.;
        let angle = pose.angle;
        let opacity = pose.opacity;
        let radius = (card_size * 0.28).min(6.);
        let glyph_size = card_size * icon.glyph_scale();
        let glyph_x = x + (card_width - glyph_size) / 2.;
        let glyph_y = y + (card_size - glyph_size) / 2.;
        let art_x = x + 1.15;
        let art_y = y + 1.15;
        let art_width = card_width - 2.3;
        let art_height = card_size - 2.3;
        let background = hex(icon.background(dark));
        let foreground = hex(icon.foreground(dark, true));
        let artwork = svg_body(icon);
        let outer =
            eggie_ui_kit::continuous_corners::svg_rect_path(x, y, card_width, card_size, radius);
        let shadow = eggie_ui_kit::continuous_corners::svg_rect_path(
            x,
            y + 1.5,
            card_width,
            card_size,
            radius,
        );
        let inner = eggie_ui_kit::continuous_corners::svg_rect_path(
            art_x,
            art_y,
            art_width,
            art_height,
            radius - 1.15,
        );
        body.push_str(&format!(
                r##"<g opacity="{opacity}" transform="rotate({angle} {center_x} {center_y})">
  <path d="{shadow}" fill="#111820" fill-opacity=".16" filter="url(#softShadow)"/>
  <path d="{outer}" fill="#172025" fill-opacity=".32" filter="url(#edgeShadow)"/>
  <path d="{outer}" fill="#FFFFFF"/>
  <path d="{inner}" fill="#172025" fill-opacity=".20" filter="url(#innerShadow)"/>
  <path d="{inner}" fill="{background}"/>
  <path d="{inner}" fill="url(#spotlight)"/>
  <path d="{inner}" fill="url(#bottomShade)"/>
  <svg x="{glyph_x}" y="{glyph_y}" width="{glyph_size}" height="{glyph_size}" viewBox="0 0 24 24" fill="{foreground}">{artwork}</svg>
</g>"##,
                center_x = x + card_width / 2.,
                center_y = y + card_size / 2.,
            ));
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">
<defs>
  <radialGradient id="spotlight" cx="50%" cy="-10%" r="85%">
    <stop offset="0" stop-color="#FFFFFF" stop-opacity=".32"/>
    <stop offset=".55" stop-color="#FFFFFF" stop-opacity=".08"/>
    <stop offset="1" stop-color="#FFFFFF" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="bottomShade" x1="0%" y1="0%" x2="0%" y2="100%">
    <stop offset="0" stop-color="#172025" stop-opacity="0"/>
    <stop offset=".75" stop-color="#172025" stop-opacity="0"/>
    <stop offset="1" stop-color="#172025" stop-opacity=".02"/>
  </linearGradient>
  <filter id="softShadow" x="-25%" y="-25%" width="150%" height="160%"><feGaussianBlur stdDeviation="1.1"/></filter>
  <filter id="edgeShadow" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation=".4"/></filter>
  <filter id="innerShadow" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation=".4"/></filter>
</defs>
{body}
</svg>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(name: &str, args: &[&str]) -> ProcessSummary {
        ProcessSummary {
            pid: 1,
            name: name.into(),
            command_line: Some(args.iter().map(|arg| (*arg).into()).collect()),
        }
    }

    #[test]
    fn foreground_executable_and_git_subcommand_choose_icons() {
        assert_eq!(
            for_process(&process("/usr/bin/python3.12", &["python3.12"])),
            ProcessIcon::Python
        );
        assert_eq!(
            for_process(&process("git", &["git", "status"])),
            ProcessIcon::Git
        );
        assert_eq!(
            for_process(&process("git", &["git", "-C", "/repo", "diff", "--cached"])),
            ProcessIcon::GitDiff
        );
        assert_eq!(
            for_process(&process("node", &["node", "/opt/claude-code/cli.js"])),
            ProcessIcon::ClaudeCode
        );
        for (name, args) in [
            ("/usr/local/bin/pi", vec!["pi"]),
            ("node", vec!["node", "/usr/local/bin/pi"]),
            (
                "node",
                vec![
                    "node",
                    "--no-warnings",
                    "/opt/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
                ],
            ),
            (
                "bun",
                vec![
                    "bun",
                    "/opt/node_modules/@mariozechner/pi-coding-agent/dist/cli.js",
                ],
            ),
        ] {
            assert_eq!(for_process(&process(name, &args)), ProcessIcon::PiAgent);
        }
        assert_eq!(
            for_process(&process("node", &["node", "server.js", "--prompt", "pi"])),
            ProcessIcon::NodeJs
        );
        assert_eq!(
            for_process(&process("zsh", &["zsh"])),
            ProcessIcon::Terminal
        );
    }

    #[test]
    fn shell_command_identifies_remote_process_but_does_not_override_local_foreground() {
        let mut session = SessionSummary {
            id: uuid::Uuid::nil(),
            project_id: uuid::Uuid::nil(),
            title: String::new(),
            initial_directory: "/".into(),
            current_directory: "/".into(),
            reported_location: None,
            shell_pid: 1,
            current_process: process("ssh", &["ssh", "example.com"]),
            active_command: Some("git diff --stat".into()),
            program_status: None,
            status: eggie_protocol::SessionStatus::Running,
        };
        assert_eq!(for_session(&session), ProcessIcon::GitDiff);
        session.active_command = Some("claude".into());
        assert_eq!(for_session(&session), ProcessIcon::ClaudeCode);
        session.active_command = Some("pi".into());
        assert_eq!(for_session(&session), ProcessIcon::PiAgent);
        session.active_command = None;
        session.program_status = Some(eggie_protocol::TerminalProgramStatus {
            state: eggie_protocol::TerminalProgramState::Working,
            kind: None,
            app: Some("claude-code".into()),
            title: None,
            message: None,
            progress: None,
        });
        assert_eq!(for_session(&session), ProcessIcon::ClaudeCode);
        session.program_status.as_mut().unwrap().app = Some("pi-agent".into());
        assert_eq!(for_session(&session), ProcessIcon::PiAgent);
        session.program_status = None;
        session.current_process = process("node", &["node", "server.js"]);
        assert_eq!(for_session(&session), ProcessIcon::NodeJs);
    }

    #[test]
    fn a_single_card_has_no_rotation() {
        let single = card_stack(&[ProcessIcon::Git], false, 20.).unwrap();
        let stacked = card_stack(&[ProcessIcon::Git, ProcessIcon::NodeJs], false, 20.).unwrap();
        assert!(
            !std::str::from_utf8(single.bytes())
                .unwrap()
                .contains("rotate(-7")
        );
        assert!(
            std::str::from_utf8(stacked.bytes())
                .unwrap()
                .contains("rotate(-7")
        );
    }

    #[test]
    fn embedded_icons_render_with_gpui_svg_renderer() {
        let renderer = gpui::SvgRenderer::new(Arc::new(()));
        let icons = [
            ProcessIcon::ProjectPlaceholder,
            ProcessIcon::Terminal,
            ProcessIcon::NodeJs,
            ProcessIcon::Python,
            ProcessIcon::Git,
            ProcessIcon::GitDiff,
            ProcessIcon::Go,
            ProcessIcon::Rust,
            ProcessIcon::BunJs,
            ProcessIcon::Zig,
            ProcessIcon::Codex,
            ProcessIcon::ClaudeCode,
            ProcessIcon::OpenCode,
            ProcessIcon::PiAgent,
            ProcessIcon::File,
            ProcessIcon::Browser,
            ProcessIcon::Settings,
        ];
        for icon in icons {
            let card = card_stack(&[icon], false, 18.).unwrap();
            renderer
                .render_single_frame(card.bytes(), 1.)
                .expect("card renders");
        }
        let stack = card_stack(&icons[1..4], false, 21.).unwrap();
        renderer
            .render_single_frame(stack.bytes(), 1.)
            .expect("stack renders");
        for dark in [false, true] {
            let empty = card_stack_frame(&[ProcessIcon::ProjectPlaceholder], dark, 21.).unwrap();
            assert!(!empty.as_bytes(0).unwrap().is_empty());
        }
        let moving = card_frame(
            &[
                CardPose {
                    icon: ProcessIcon::BunJs,
                    offset_x: 2.5,
                    angle: 3.5,
                    opacity: 0.5,
                },
                CardPose {
                    icon: ProcessIcon::Terminal,
                    offset_x: 0.,
                    angle: -3.5,
                    opacity: 1.,
                },
            ],
            false,
            21.,
            stack_width(2, 21.),
        );
        assert!(!moving.as_bytes(0).unwrap().is_empty());
    }
}
