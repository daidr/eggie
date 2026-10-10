use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    #[default]
    Terminal,
    File,
    Browser,
    Settings,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabItem {
    pub id: ItemId,
    pub kind: ItemKind,
    /// The process/OSC-derived title. Kept updated by the running shell and never reflects a
    /// user override, so removing a custom title falls back to the live process title.
    pub title: String,
    /// A user-assigned override that wins over `title` when set. Independent metadata: OSC title
    /// updates leave it untouched, and clearing it (`None`) restores the process title.
    #[serde(default)]
    pub custom_title: Option<String>,
    pub session_id: Option<SessionId>,
    /// The profile this terminal was launched from, recorded so the tab can show its badge and so
    /// appearance stays resolvable while the terminal lives. Locked at creation: the launch fields
    /// are already baked into the running shell, so re-pointing a live tab at another profile would
    /// only ever be half-true. `None` for items that predate profiles or are not terminals.
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

impl TabItem {
    pub fn terminal(session_id: SessionId, title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: ItemKind::Terminal,
            title: title.into(),
            custom_title: None,
            session_id: Some(session_id),
            profile_id: None,
        }
    }

    /// A terminal tab launched from `profile_id`.
    pub fn terminal_with_profile(
        session_id: SessionId,
        title: impl Into<String>,
        profile_id: ProfileId,
    ) -> Self {
        Self {
            profile_id: Some(profile_id),
            ..Self::terminal(session_id, title)
        }
    }

    /// The title to render: the user's custom title if set, otherwise the process title.
    pub fn display_title(&self) -> &str {
        self.custom_title.as_deref().unwrap_or(&self.title)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    pub fn axis(self) -> SplitAxis {
        match self {
            Self::Left | Self::Right => SplitAxis::Horizontal,
            Self::Up | Self::Down => SplitAxis::Vertical,
        }
    }

    fn inserts_before(self) -> bool {
        matches!(self, Self::Up | Self::Left)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: GroupId,
    pub items: Vec<TabItem>,
    pub active_item_id: Option<ItemId>,
}

impl TabGroup {
    pub fn new(item: TabItem) -> Self {
        let active_item_id = Some(item.id);
        Self {
            id: Uuid::new_v4(),
            items: vec![item],
            active_item_id,
        }
    }

    pub fn add_item(&mut self, item: TabItem) {
        self.active_item_id = Some(item.id);
        self.items.push(item);
    }

    pub fn insert_item(&mut self, index: usize, item: TabItem) {
        self.active_item_id = Some(item.id);
        self.items.insert(index.min(self.items.len()), item);
    }

    pub fn close_item(&mut self, item_id: ItemId) -> Option<TabItem> {
        let index = self.items.iter().position(|item| item.id == item_id)?;
        let removed = self.items.remove(index);
        if self.active_item_id == Some(item_id) {
            self.active_item_id = self
                .items
                .get(index.min(self.items.len().saturating_sub(1)))
                .map(|item| item.id);
        }
        Some(removed)
    }

    pub fn active_item(&self) -> Option<&TabItem> {
        let active = self.active_item_id?;
        self.items.iter().find(|item| item.id == active)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    Group {
        group: TabGroup,
    },
    Split {
        /// Stable, globally unique identity for this split.
        ///
        /// A split must NOT be identified by the first group inside its `first` subtree: when the
        /// `first` subtree is itself a split, the outer and inner splits would derive the same
        /// "first group" and collide, so resizing the inner divider would move the outer one. This
        /// explicit id sidesteps that entirely.
        #[serde(default = "Uuid::new_v4")]
        id: SplitId,
        axis: SplitAxis,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    pub fn group(item: TabItem) -> Self {
        Self::Group {
            group: TabGroup::new(item),
        }
    }

    /// A layout with a single empty group and no tabs. Used for a project that currently has no
    /// terminal in this window (e.g. a project synced from another window, or one whose sessions
    /// were all detached): the group is a valid drop target for the first new terminal.
    pub fn empty() -> Self {
        Self::Group {
            group: TabGroup {
                id: Uuid::new_v4(),
                items: Vec::new(),
                active_item_id: None,
            },
        }
    }

    pub fn find_group_mut(&mut self, group_id: GroupId) -> Option<&mut TabGroup> {
        match self {
            Self::Group { group } => (group.id == group_id).then_some(group),
            Self::Split { first, second, .. } => first
                .find_group_mut(group_id)
                .or_else(|| second.find_group_mut(group_id)),
        }
    }

    pub fn find_group(&self, group_id: GroupId) -> Option<&TabGroup> {
        match self {
            Self::Group { group } => (group.id == group_id).then_some(group),
            Self::Split { first, second, .. } => first
                .find_group(group_id)
                .or_else(|| second.find_group(group_id)),
        }
    }

    pub fn find_terminal_item(&self, session_id: SessionId) -> Option<(GroupId, ItemId)> {
        match self {
            Self::Group { group } => group
                .items
                .iter()
                .find(|item| item.session_id == Some(session_id))
                .map(|item| (group.id, item.id)),
            Self::Split { first, second, .. } => first
                .find_terminal_item(session_id)
                .or_else(|| second.find_terminal_item(session_id)),
        }
    }

    /// Updates the title of every tab backed by `session_id`.
    ///
    /// A session can move between groups while its terminal process keeps running, so the
    /// session ID—not the current group or item ID—is the stable identity for OSC title updates.
    pub fn update_terminal_title(&mut self, session_id: SessionId, title: &str) -> bool {
        match self {
            Self::Group { group } => {
                let mut changed = false;
                for item in &mut group.items {
                    if item.session_id == Some(session_id) && item.title != title {
                        item.title = title.to_owned();
                        changed = true;
                    }
                }
                changed
            }
            Self::Split { first, second, .. } => {
                let first_changed = first.update_terminal_title(session_id, title);
                let second_changed = second.update_terminal_title(session_id, title);
                first_changed || second_changed
            }
        }
    }

    /// The title to display for the tab backed by `session_id`: the tab's custom title if set,
    /// otherwise its process title. Returns `None` when no tab is backed by that session.
    ///
    /// Used by surfaces outside the tab bar (e.g. the sidebar session list) that key off a session
    /// id rather than a tab, so a user's custom tab title stays consistent everywhere.
    pub fn terminal_display_title(&self, session_id: SessionId) -> Option<&str> {
        match self {
            Self::Group { group } => group
                .items
                .iter()
                .find(|item| item.session_id == Some(session_id))
                .map(|item| item.display_title()),
            Self::Split { first, second, .. } => first
                .terminal_display_title(session_id)
                .or_else(|| second.terminal_display_title(session_id)),
        }
    }

    /// Returns a user's title override for the terminal, when one is set.
    pub fn terminal_custom_title(&self, session_id: SessionId) -> Option<&str> {
        match self {
            Self::Group { group } => group
                .items
                .iter()
                .find(|item| item.session_id == Some(session_id))
                .and_then(|item| item.custom_title.as_deref()),
            Self::Split { first, second, .. } => first
                .terminal_custom_title(session_id)
                .or_else(|| second.terminal_custom_title(session_id)),
        }
    }

    /// Sets (or clears, with `None`) the user's custom title override for a single tab, identified
    /// by its group and item id. Returns `true` if the override actually changed.
    ///
    /// Unlike [`Self::update_terminal_title`], this targets one specific item rather than every tab
    /// backed by a session: a custom title is per-tab metadata, not a process property.
    pub fn set_custom_title(
        &mut self,
        group_id: GroupId,
        item_id: ItemId,
        custom_title: Option<String>,
    ) -> bool {
        let Some(group) = self.find_group_mut(group_id) else {
            return false;
        };
        let Some(item) = group.items.iter_mut().find(|item| item.id == item_id) else {
            return false;
        };
        if item.custom_title == custom_title {
            return false;
        }
        item.custom_title = custom_title;
        true
    }

    pub fn first_group_id(&self) -> GroupId {
        match self {
            Self::Group { group } => group.id,
            Self::Split { first, .. } => first.first_group_id(),
        }
    }

    /// Returns this node's [`SplitId`] if it is a split, or `None` for a group.
    pub fn split_id(&self) -> Option<SplitId> {
        match self {
            Self::Group { .. } => None,
            Self::Split { id, .. } => Some(*id),
        }
    }

    /// Returns the ratio of the split with the given [`SplitId`].
    pub fn split_ratio(&self, split_id: SplitId) -> Option<f32> {
        match self {
            Self::Group { .. } => None,
            Self::Split { id, ratio, .. } if *id == split_id => Some(*ratio),
            Self::Split { first, second, .. } => first
                .split_ratio(split_id)
                .or_else(|| second.split_ratio(split_id)),
        }
    }

    /// Returns the axis of the split with the given [`SplitId`].
    pub fn split_axis(&self, split_id: SplitId) -> Option<SplitAxis> {
        match self {
            Self::Group { .. } => None,
            Self::Split { id, axis, .. } if *id == split_id => Some(*axis),
            Self::Split { first, second, .. } => first
                .split_axis(split_id)
                .or_else(|| second.split_axis(split_id)),
        }
    }

    /// Updates the ratio of the split with the given [`SplitId`].
    ///
    /// Returns `true` if a matching split was found and updated. The ratio is clamped to
    /// `0.05..=0.95` so neither pane can collapse entirely.
    pub fn set_split_ratio(&mut self, split_id: SplitId, ratio: f32) -> bool {
        let ratio = ratio.clamp(0.05, 0.95);
        match self {
            Self::Group { .. } => false,
            Self::Split {
                id, ratio: current, ..
            } if *id == split_id => {
                *current = ratio;
                true
            }
            Self::Split { first, second, .. } => {
                first.set_split_ratio(split_id, ratio) || second.set_split_ratio(split_id, ratio)
            }
        }
    }

    pub fn top_left_group_id(&self) -> GroupId {
        match self {
            Self::Group { group } => group.id,
            Self::Split { first, .. } => first.top_left_group_id(),
        }
    }

    pub fn top_right_group_id(&self) -> GroupId {
        match self {
            Self::Group { group } => group.id,
            Self::Split {
                axis: SplitAxis::Horizontal,
                second,
                ..
            } => second.top_right_group_id(),
            Self::Split {
                axis: SplitAxis::Vertical,
                first,
                ..
            } => first.top_right_group_id(),
        }
    }

    pub fn split_group(
        &mut self,
        group_id: GroupId,
        axis: SplitAxis,
        item: TabItem,
    ) -> Option<GroupId> {
        let direction = match axis {
            SplitAxis::Horizontal => Direction::Right,
            SplitAxis::Vertical => Direction::Down,
        };
        self.split_group_direction(group_id, direction, item)
    }

    pub fn split_group_direction(
        &mut self,
        group_id: GroupId,
        direction: Direction,
        item: TabItem,
    ) -> Option<GroupId> {
        match self {
            Self::Group { group } if group.id == group_id => {
                let replacement = Self::Group {
                    group: TabGroup {
                        id: group.id,
                        items: std::mem::take(&mut group.items),
                        active_item_id: group.active_item_id.take(),
                    },
                };
                let new_group = TabGroup::new(item);
                let new_group_id = new_group.id;
                let old_group = Box::new(replacement);
                let new_group = Box::new(Self::Group { group: new_group });
                let (first, second) = if direction.inserts_before() {
                    (new_group, old_group)
                } else {
                    (old_group, new_group)
                };
                *self = Self::Split {
                    id: Uuid::new_v4(),
                    axis: direction.axis(),
                    ratio: 0.5,
                    first,
                    second,
                };
                Some(new_group_id)
            }
            Self::Group { .. } => None,
            Self::Split { first, second, .. } => first
                .split_group_direction(group_id, direction, item.clone())
                .or_else(|| second.split_group_direction(group_id, direction, item)),
        }
    }

    pub fn move_item(
        &mut self,
        source_group_id: GroupId,
        item_id: ItemId,
        target_group_id: GroupId,
    ) -> Option<GroupId> {
        if source_group_id == target_group_id {
            let group = self.find_group_mut(source_group_id)?;
            group.active_item_id = Some(item_id);
            return Some(source_group_id);
        }
        self.find_group(target_group_id)?;
        let item = self.close_item(source_group_id, item_id)?;
        self.find_group_mut(target_group_id)?.add_item(item);
        Some(target_group_id)
    }

    pub fn move_item_at(
        &mut self,
        source_group_id: GroupId,
        item_id: ItemId,
        target_group_id: GroupId,
        insertion_index: usize,
    ) -> Option<GroupId> {
        if source_group_id == target_group_id {
            let group = self.find_group_mut(source_group_id)?;
            let source_index = group.items.iter().position(|item| item.id == item_id)?;
            let insertion_index = insertion_index.min(group.items.len());
            let item = group.items.remove(source_index);
            let adjusted_index = if insertion_index > source_index {
                insertion_index - 1
            } else {
                insertion_index
            };
            group.insert_item(adjusted_index, item);
            return Some(source_group_id);
        }

        let target_length = self.find_group(target_group_id)?.items.len();
        let item = self.close_item(source_group_id, item_id)?;
        self.find_group_mut(target_group_id)?
            .insert_item(insertion_index.min(target_length), item);
        Some(target_group_id)
    }

    pub fn move_item_to_new_group(
        &mut self,
        source_group_id: GroupId,
        item_id: ItemId,
        target_group_id: GroupId,
        direction: Direction,
    ) -> Option<GroupId> {
        self.find_group(target_group_id)?;
        let source_item_count = self.find_group(source_group_id)?.items.len();
        if source_group_id == target_group_id && source_item_count == 1 {
            return None;
        }

        let item = self.close_item(source_group_id, item_id)?;
        self.split_group_direction(target_group_id, direction, item)
    }

    pub fn neighbor_group_id(
        &self,
        source_group_id: GroupId,
        direction: Direction,
    ) -> Option<GroupId> {
        let mut groups = Vec::new();
        self.collect_group_rects(NormalizedRect::FULL, &mut groups);
        let source = groups
            .iter()
            .find(|(group_id, _)| *group_id == source_group_id)
            .map(|(_, rect)| *rect)?;

        groups
            .into_iter()
            .filter(|(group_id, _)| *group_id != source_group_id)
            .filter_map(|(group_id, candidate)| {
                let primary_distance = match direction {
                    Direction::Up if candidate.center_y() < source.center_y() => {
                        source.center_y() - candidate.center_y()
                    }
                    Direction::Down if candidate.center_y() > source.center_y() => {
                        candidate.center_y() - source.center_y()
                    }
                    Direction::Left if candidate.center_x() < source.center_x() => {
                        source.center_x() - candidate.center_x()
                    }
                    Direction::Right if candidate.center_x() > source.center_x() => {
                        candidate.center_x() - source.center_x()
                    }
                    _ => return None,
                };
                let cross_axis_gap = match direction {
                    Direction::Up | Direction::Down => interval_gap(
                        source.left,
                        source.right(),
                        candidate.left,
                        candidate.right(),
                    ),
                    Direction::Left | Direction::Right => interval_gap(
                        source.top,
                        source.bottom(),
                        candidate.top,
                        candidate.bottom(),
                    ),
                };
                Some((group_id, primary_distance + cross_axis_gap * 4.))
            })
            .min_by(|(_, first), (_, second)| first.total_cmp(second))
            .map(|(group_id, _)| group_id)
    }

    pub fn group_ids(&self) -> Vec<GroupId> {
        let mut groups = Vec::new();
        self.collect_group_rects(NormalizedRect::FULL, &mut groups);
        groups.into_iter().map(|(group_id, _)| group_id).collect()
    }

    /// The [`SplitId`]s of every split in this layout tree. Mirrors [`group_ids`]; used by the UI to
    /// retain per-split render caches (divider bounds) against the live layout so closed splits do
    /// not leak stale entries.
    pub fn split_ids(&self) -> Vec<SplitId> {
        let mut splits = Vec::new();
        self.collect_split_ids(&mut splits);
        splits
    }

    fn collect_split_ids(&self, splits: &mut Vec<SplitId>) {
        if let Self::Split {
            id, first, second, ..
        } = self
        {
            splits.push(*id);
            first.collect_split_ids(splits);
            second.collect_split_ids(splits);
        }
    }

    /// Total number of tab items across all groups in this layout.
    pub fn item_count(&self) -> usize {
        match self {
            Self::Group { group } => group.items.len(),
            Self::Split { first, second, .. } => first.item_count() + second.item_count(),
        }
    }

    /// Iterate over all tab items across all groups, in layout order.
    pub fn items(&self) -> impl Iterator<Item = &TabItem> {
        let mut items = Vec::new();
        self.collect_items(&mut items);
        items.into_iter()
    }

    /// Iterate over terminal tabs across all groups.
    pub fn terminal_items(&self) -> impl Iterator<Item = &TabItem> {
        self.items()
            .filter(|item| matches!(item.kind, ItemKind::Terminal))
    }

    fn collect_items<'a>(&'a self, items: &mut Vec<&'a TabItem>) {
        match self {
            Self::Group { group } => {
                items.extend(&group.items);
            }
            Self::Split { first, second, .. } => {
                first.collect_items(items);
                second.collect_items(items);
            }
        }
    }

    fn collect_group_rects(
        &self,
        rect: NormalizedRect,
        groups: &mut Vec<(GroupId, NormalizedRect)>,
    ) {
        match self {
            Self::Group { group } => groups.push((group.id, rect)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
                ..
            } => {
                let ratio = ratio.clamp(0.05, 0.95);
                let (first_rect, second_rect) = rect.split(*axis, ratio);
                first.collect_group_rects(first_rect, groups);
                second.collect_group_rects(second_rect, groups);
            }
        }
    }

    pub fn close_item(&mut self, group_id: GroupId, item_id: ItemId) -> Option<TabItem> {
        let removed = self.find_group_mut(group_id)?.close_item(item_id)?;
        let group_is_empty = self
            .find_group(group_id)
            .is_some_and(|group| group.items.is_empty());
        if group_is_empty {
            let placeholder = Self::empty_group(group_id);
            let current = std::mem::replace(self, placeholder);
            if let Some(pruned) = current.without_empty_group(group_id) {
                *self = pruned;
            }
        }
        Some(removed)
    }

    fn empty_group(group_id: GroupId) -> Self {
        Self::Group {
            group: TabGroup {
                id: group_id,
                items: Vec::new(),
                active_item_id: None,
            },
        }
    }

    fn without_empty_group(self, group_id: GroupId) -> Option<Self> {
        match self {
            Self::Group { group } if group.id == group_id && group.items.is_empty() => None,
            Self::Group { .. } => Some(self),
            Self::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => match (
                first.without_empty_group(group_id),
                second.without_empty_group(group_id),
            ) {
                // Both subtrees survive: keep this split with its original id so a live drag
                // targeting it is not invalidated by an unrelated group closing elsewhere.
                (Some(first), Some(second)) => Some(Self::Split {
                    id,
                    axis,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct NormalizedRect {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl NormalizedRect {
    const FULL: Self = Self {
        left: 0.,
        top: 0.,
        width: 1.,
        height: 1.,
    };

    fn right(self) -> f32 {
        self.left + self.width
    }

    fn bottom(self) -> f32 {
        self.top + self.height
    }

    fn center_x(self) -> f32 {
        self.left + self.width / 2.
    }

    fn center_y(self) -> f32 {
        self.top + self.height / 2.
    }

    fn split(self, axis: SplitAxis, ratio: f32) -> (Self, Self) {
        match axis {
            SplitAxis::Horizontal => {
                let first_width = self.width * ratio;
                (
                    Self {
                        width: first_width,
                        ..self
                    },
                    Self {
                        left: self.left + first_width,
                        width: self.width - first_width,
                        ..self
                    },
                )
            }
            SplitAxis::Vertical => {
                let first_height = self.height * ratio;
                (
                    Self {
                        height: first_height,
                        ..self
                    },
                    Self {
                        top: self.top + first_height,
                        height: self.height - first_height,
                        ..self
                    },
                )
            }
        }
    }
}

fn interval_gap(first_start: f32, first_end: f32, second_start: f32, second_end: f32) -> f32 {
    if first_end < second_start {
        second_start - first_end
    } else if second_end < first_start {
        first_start - second_end
    } else {
        0.
    }
}
