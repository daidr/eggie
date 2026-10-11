use super::*;

/// Direction for jump-to-prompt navigation (daemon-internal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalJumpDirection {
    /// Toward older prompts (further up in scrollback).
    Up,
    /// Toward newer prompts (further down toward the live bottom).
    Down,
}

pub(super) struct ShellIntegrationTracker {
    pub(super) prompt_input: Option<eggie_completion::PromptInput>,
    pub(super) input_starts: Vec<(u64, usize)>,
    pub(super) completion_shell: String,
    pub(super) completion_native: bool,
    pub(super) completion_environment: Option<eggie_completion::PromptInput>,
    pub(super) phase: TerminalSemanticPhase,
    /// Grid line where the active prompt started (relative to the top of the visible screen, as
    /// reported with the OSC 133 A/prompt-start marker). Used to clear exactly the prompt region on
    /// resize. `None` when not currently on a prompt.
    pub(super) prompt_start_line: Option<i32>,
    pub(super) current_command: Option<TerminalCommandRecord>,
    pub(super) history: VecDeque<TerminalCommandRecord>,
    pub(super) next_command_id: u64,
    pub(super) user_variables: HashMap<String, String>,
    /// Monotonic count of lines that have scrolled off the top of the active screen since the
    /// session started. Kept fresh each frame by [`ShellIntegrationTracker::observe_scroll`]. Used
    /// as a stable coordinate base: a prompt captured at active-screen line `cursor_line` is stored
    /// as `total_scrolled_lines + cursor_line`, which never shifts as scrollback grows.
    pub(super) total_scrolled_lines: u64,
    /// The `history_size` seen at the previous `observe_scroll`, to derive scroll increments after
    /// the scrollback saturates (once at the limit, `history_size` stops growing).
    pub(super) last_history_size: usize,
    /// Global line coordinates (see `total_scrolled_lines`) of each recorded prompt start, oldest
    /// first. This is the jump-to-prompt index. Bounded by `COMMAND_HISTORY` and pruned as
    /// scrollback evicts old lines.
    pub(super) prompt_jump_points: VecDeque<u64>,
}

impl Default for ShellIntegrationTracker {
    fn default() -> Self {
        Self {
            prompt_input: None,
            input_starts: Vec::new(),
            completion_shell: String::new(),
            completion_native: false,
            completion_environment: None,
            phase: TerminalSemanticPhase::None,
            prompt_start_line: None,
            current_command: None,
            history: VecDeque::with_capacity(COMMAND_HISTORY),
            next_command_id: 1,
            user_variables: HashMap::new(),
            total_scrolled_lines: 0,
            last_history_size: 0,
            prompt_jump_points: VecDeque::new(),
        }
    }
}

impl ShellIntegrationTracker {
    /// Update the tracker from an OSC 133 marker. `cursor_line` is the grid line the cursor was on
    /// when the marker was emitted (relative to the top of the visible screen), used to record
    /// where the active prompt starts. `history_size` is the scrollback depth captured at the same
    /// instant (synchronously, under the terminal lock) — folding it in here keeps the jump-point
    /// coordinate accurate even when the throttled `observe_scroll` has not caught up with a burst
    /// of output.
    pub(super) fn update(&mut self, prompt: SemanticPrompt, cursor_line: i32, history_size: usize) {
        let now = unix_time_ms();
        // Fold the freshly-captured scrollback depth into the coordinate base before recording any
        // jump point, so `record_prompt_jump_point` uses an up-to-date `total_scrolled_lines`.
        self.advance_scroll_base(history_size);
        if matches!(
            prompt.action,
            SemanticPromptAction::FreshLineAndPrompt | SemanticPromptAction::PromptStart
        ) {
            self.input_starts.clear();
            self.prompt_input = None;
        }
        match prompt.action {
            SemanticPromptAction::FreshLine => {}
            SemanticPromptAction::FreshLineAndPrompt | SemanticPromptAction::PromptStart => {
                // A/prompt-start marks where the prompt begins. Record it only when entering the
                // prompt from a non-prompt phase, so continuation/re-emitted marks on the same
                // prompt (e.g. our zle-line-init hook firing on every redraw) don't move the start
                // down to the input line.
                if !matches!(
                    self.phase,
                    TerminalSemanticPhase::Prompt | TerminalSemanticPhase::Input
                ) {
                    self.prompt_start_line = Some(cursor_line);
                    self.record_prompt_jump_point(cursor_line);
                }
                self.phase = TerminalSemanticPhase::Prompt;
            }
            SemanticPromptAction::NewCommand => {
                self.begin_command(now, option_value(&prompt.options, "cmdline"));
                self.prompt_start_line = Some(cursor_line);
                self.record_prompt_jump_point(cursor_line);
                self.phase = TerminalSemanticPhase::Prompt;
            }
            SemanticPromptAction::InputStart
            | SemanticPromptAction::InputStartAndTerminatePrompt => {
                if self.current_command.is_none() {
                    self.begin_command(now, command_line_from_options(&prompt.options));
                }
                // Entering input directly (no preceding A) still means the cursor is on a prompt;
                // record the start if we don't have one yet.
                if self.prompt_start_line.is_none() {
                    self.prompt_start_line = Some(cursor_line);
                }
                self.phase = TerminalSemanticPhase::Input;
            }
            SemanticPromptAction::OutputStart => {
                let command_line = command_line_from_options(&prompt.options);
                if self.current_command.is_none() {
                    self.begin_command(now, command_line);
                } else if let Some(command_line) = command_line
                    && let Some(command) = self.current_command.as_mut()
                {
                    command.command_line = Some(command_line);
                }
                // Output has started; the prompt is no longer the active region.
                self.prompt_start_line = None;
                self.prompt_input = None;
                self.input_starts.clear();
                self.phase = TerminalSemanticPhase::Output;
            }
            SemanticPromptAction::CommandFinished => {
                if !matches!(
                    self.phase,
                    TerminalSemanticPhase::Input | TerminalSemanticPhase::Output
                ) {
                    return;
                }
                let exit_code = prompt
                    .options
                    .split(';')
                    .next()
                    .filter(|value| !value.is_empty())
                    .and_then(|value| value.parse::<i32>().ok());
                if self.phase == TerminalSemanticPhase::Input {
                    self.current_command = None;
                } else if let Some(mut command) = self.current_command.take() {
                    command.completed_at_unix_ms = Some(now);
                    command.exit_code = exit_code.filter(|code| (0..=255).contains(code));
                    self.history.push_back(command);
                    if self.history.len() > COMMAND_HISTORY {
                        self.history.pop_front();
                    }
                }
                self.prompt_start_line = None;
                self.prompt_input = None;
                self.input_starts.clear();
                self.phase = TerminalSemanticPhase::None;
            }
        }
    }

    pub(super) fn begin_command(&mut self, now: u64, command_line: Option<String>) {
        if let Some(mut previous) = self.current_command.take() {
            previous.completed_at_unix_ms = Some(now);
            self.history.push_back(previous);
        }
        self.current_command = Some(TerminalCommandRecord {
            id: self.next_command_id,
            command_line,
            started_at_unix_ms: now,
            completed_at_unix_ms: None,
            exit_code: None,
        });
        self.next_command_id = self.next_command_id.wrapping_add(1).max(1);
        while self.history.len() > COMMAND_HISTORY {
            self.history.pop_front();
        }
    }

    /// Record the global line coordinate of a prompt start for jump-to-prompt. De-dupes a repeat of
    /// the current prompt (same global line) rather than appending a second jump point.
    pub(super) fn record_prompt_jump_point(&mut self, cursor_line: i32) {
        let global = self.total_scrolled_lines as i64 + cursor_line as i64;
        if global < 0 {
            return;
        }
        let global = global as u64;
        if self.prompt_jump_points.back() == Some(&global) {
            return;
        }
        self.prompt_jump_points.push_back(global);
        while self.prompt_jump_points.len() > COMMAND_HISTORY {
            self.prompt_jump_points.pop_front();
        }
    }

    /// Advance the scroll-base (`total_scrolled_lines`) toward the given live `history_size`.
    /// Shared by the OSC-133 capture path (accurate at marker time) and the per-frame
    /// `observe_scroll`. While unsaturated, `history_size` equals the lines scrolled off the top, so
    /// the base tracks it exactly; after saturation `history_size` pins to the limit and this
    /// advances by the observed increase (best effort). A shrink (clear/reset) rebases downward.
    pub(super) fn advance_scroll_base(&mut self, history_size: usize) {
        if history_size >= self.last_history_size {
            self.total_scrolled_lines += (history_size - self.last_history_size) as u64;
        } else {
            self.total_scrolled_lines = self
                .total_scrolled_lines
                .saturating_sub((self.last_history_size - history_size) as u64);
        }
        self.last_history_size = history_size;
    }

    /// Keep `total_scrolled_lines` current and prune jump points that have scrolled out of the
    /// buffer. Called once per published frame with the terminal's live `history_size`.
    pub(super) fn observe_scroll(&mut self, history_size: usize, scrollback_limit: usize) {
        self.advance_scroll_base(history_size);
        let _ = scrollback_limit;
        // The oldest visible-or-scrollback line has global coordinate `total_scrolled - history`.
        // Anything older has been evicted from the buffer and can never be jumped to again.
        let oldest_live = self
            .total_scrolled_lines
            .saturating_sub(history_size as u64);
        while self
            .prompt_jump_points
            .front()
            .is_some_and(|&global| global < oldest_live)
        {
            self.prompt_jump_points.pop_front();
        }
    }

    /// Reset the jump-point index and its coordinate base. Used when a column resize reflows the
    /// buffer, invalidating every stored line coordinate.
    pub(super) fn clear_jump_points(&mut self) {
        self.prompt_jump_points.clear();
        self.total_scrolled_lines = 0;
        self.last_history_size = 0;
    }

    /// Pick the jump target relative to the line currently at the top of the viewport.
    /// `viewport_top_global` is that line's global coordinate. Returns the target's global
    /// coordinate, or `None` when there is no strictly-earlier / strictly-later prompt.
    pub(super) fn jump_target(
        &self,
        viewport_top_global: i64,
        direction: TerminalJumpDirection,
    ) -> Option<u64> {
        match direction {
            TerminalJumpDirection::Up => self
                .prompt_jump_points
                .iter()
                .rev()
                .find(|&&global| (global as i64) < viewport_top_global)
                .copied(),
            TerminalJumpDirection::Down => self
                .prompt_jump_points
                .iter()
                .find(|&&global| (global as i64) > viewport_top_global)
                .copied(),
        }
    }

    pub(super) fn snapshot(&self) -> TerminalShellIntegrationState {
        let mut user_variables = self
            .user_variables
            .iter()
            .map(|(name, value)| TerminalUserVariable {
                name: name.clone(),
                value: value.clone(),
            })
            .collect::<Vec<_>>();
        user_variables.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        TerminalShellIntegrationState {
            phase: self.phase,
            current_command: self.current_command.clone(),
            history: self.history.iter().cloned().collect(),
            user_variables,
        }
    }
}
