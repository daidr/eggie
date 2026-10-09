use eggie_protocol::{ProcessSummary, SessionSummary};
use gpui::{Image, ImageFormat};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
};

/// The icon follows the session's foreground process, never the tab title (which users can edit).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ProcessIcon {
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
    File,
    Browser,
    Settings,
}

impl ProcessIcon {
    fn source(self) -> &'static str {
        match self {
            Self::Terminal => include_str!("../assets/process-icons/terminal.svg"),
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
            Self::File => include_str!("../assets/process-icons/file.svg"),
            Self::Browser => include_str!("../assets/process-icons/browser.svg"),
            Self::Settings => include_str!("../assets/process-icons/settings.svg"),
        }
    }

    fn foreground(self, dark: bool, in_card: bool) -> u32 {
        match self {
            Self::Terminal
            | Self::GitDiff
            | Self::ClaudeCode
            | Self::File
            | Self::Browser
            | Self::Settings => 0xffffff,
            Self::NodeJs => 0x3c873a,
            Self::Python => 0x3776ab,
            Self::Git => 0xf05032,
            Self::Go => 0x00add8,
            Self::Rust => 0xce422b,
            Self::BunJs => 0xbd6b4c,
            Self::Zig => 0xf7a41d,
            Self::Codex | Self::OpenCode if in_card => 0x202833,
            Self::Codex | Self::OpenCode if dark => 0xe9ecf2,
            Self::Codex | Self::OpenCode => 0x202833,
        }
    }

    fn background(self) -> u32 {
        match self {
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
            Self::ClaudeCode => 0xfbe3d5,
            Self::OpenCode => 0xe5ebec,
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

fn card_body(icon: ProcessIcon) -> &'static str {
    if icon == ProcessIcon::Terminal {
        svg_inner(include_str!("../assets/process-icons/terminal-mark.svg"))
    } else {
        svg_body(icon)
    }
}

fn hex(color: u32) -> String {
    format!("#{color:06X}")
}

fn svg_image(markup: String) -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Svg, markup.into_bytes()))
}

/// A bare glyph for the main tab strip; there is no tile or backdrop here.
pub(crate) fn bare_icon(icon: ProcessIcon, dark: bool) -> Arc<Image> {
    static CACHE: OnceLock<Mutex<HashMap<(ProcessIcon, bool), Arc<Image>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().expect("process icon cache poisoned");
    cache.entry((icon, dark)).or_insert_with(|| {
        svg_image(format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"{}\">{}</svg>",
            hex(icon.foreground(dark, false)), svg_body(icon),
        ))
    }).clone()
}

pub(crate) fn stack_width(count: usize, card_size: f32) -> f32 {
    if count == 0 {
        0.
    } else {
        card_size * 1.28 + 4. + (count.min(3) - 1) as f32 * 5.
    }
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
    Some(cache.entry(key).or_insert_with(|| {
        let width = stack_width(icons.len(), card_size);
        let height = card_size + 5.;
        let card_width = card_size * 1.28;
        let mut body = String::new();
        for index in (0..icons.len()).rev() {
            let icon = icons[index];
            let x = 2. + index as f32 * 5.;
            let y = 2.;
            let angle = if icons.len() == 1 {
                0.
            } else {
                match index {
                    0 => -7.,
                    1 if icons.len() == 2 => 7.,
                    1 => 2.,
                    _ => 9.,
                }
            };
            let radius = (card_size * 0.28).min(6.);
            let glyph_size = card_size * 0.78;
            let glyph_x = x + (card_width - glyph_size) / 2.;
            let glyph_y = y + (card_size - glyph_size) / 2.;
            let art_x = x + 1.15;
            let art_y = y + 1.15;
            let art_width = card_width - 2.3;
            let art_height = card_size - 2.3;
            let background = hex(icon.background());
            let foreground = hex(icon.foreground(dark, true));
            let artwork = card_body(icon);
            body.push_str(&format!(
                r##"<g transform="rotate({angle} {center_x} {center_y})">
  <rect x="{x}" y="{shadow_y}" width="{card_width}" height="{card_size}" rx="{radius}" fill="#111820" fill-opacity=".16" filter="url(#softShadow)"/>
  <rect x="{x}" y="{y}" width="{card_width}" height="{card_size}" rx="{radius}" fill="#172025" fill-opacity=".32" filter="url(#edgeShadow)"/>
  <rect x="{x}" y="{y}" width="{card_width}" height="{card_size}" rx="{radius}" fill="#FFFFFF"/>
  <rect x="{art_x}" y="{art_y}" width="{art_width}" height="{art_height}" rx="{art_radius}" fill="#172025" fill-opacity=".20" filter="url(#innerShadow)"/>
  <rect x="{art_x}" y="{art_y}" width="{art_width}" height="{art_height}" rx="{art_radius}" fill="{background}"/>
  <rect x="{art_x}" y="{art_y}" width="{art_width}" height="{art_height}" rx="{art_radius}" fill="url(#spotlight)"/>
  <rect x="{art_x}" y="{art_y}" width="{art_width}" height="{art_height}" rx="{art_radius}" fill="url(#bottomShade)"/>
  <svg x="{glyph_x}" y="{glyph_y}" width="{glyph_size}" height="{glyph_size}" viewBox="0 0 24 24" fill="{foreground}">{artwork}</svg>
</g>"##,
                center_x = x + card_width / 2.,
                center_y = y + card_size / 2.,
                shadow_y = y + 1.5,
                art_radius = radius - 1.15,
            ));
        }
        svg_image(format!(
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
        ))
    }).clone())
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
            window_id: None,
            title: String::new(),
            initial_directory: "/".into(),
            current_directory: "/".into(),
            reported_location: None,
            shell_pid: 1,
            current_process: process("ssh", &["ssh", "example.com"]),
            active_command: Some("git diff --stat".into()),
            status: eggie_protocol::SessionStatus::Running,
        };
        assert_eq!(for_session(&session), ProcessIcon::GitDiff);
        session.active_command = Some("claude".into());
        assert_eq!(for_session(&session), ProcessIcon::ClaudeCode);
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
            ProcessIcon::File,
            ProcessIcon::Browser,
            ProcessIcon::Settings,
        ];
        for icon in icons {
            let bare = bare_icon(icon, false);
            renderer
                .render_single_frame(bare.bytes(), 1.)
                .expect("bare icon renders");
            let card = card_stack(&[icon], false, 18.).unwrap();
            renderer
                .render_single_frame(card.bytes(), 1.)
                .expect("card renders");
        }
        let stack = card_stack(&icons[..3], false, 21.).unwrap();
        renderer
            .render_single_frame(stack.bytes(), 1.)
            .expect("stack renders");
    }
}
