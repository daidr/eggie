use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub type ProjectId = Uuid;
pub type GroupId = Uuid;
pub type ItemId = Uuid;
pub type SessionId = Uuid;
/// Identity of a GUI window. A session in the daemon is tagged with the id of the window that owns
/// it so that each window sees only its own sessions; `None` ownership marks a detached session.
pub type WindowId = Uuid;
/// Identity of a split node in a [`LayoutNode`] tree. Distinct from [`GroupId`] so a split is never
/// confused with the groups it contains.
pub type SplitId = Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    /// Default working directory for new terminals in this project. `None` means the user's home
    /// directory is used.
    pub root: Option<PathBuf>,
}

impl Project {
    pub fn from_root(root: PathBuf) -> Self {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Project")
            .to_owned();
        Self {
            id: Uuid::new_v4(),
            name,
            root: Some(root),
        }
    }

    /// Create a project with only a name and no default working directory. New terminals will
    /// start in the user's home directory until a root is set.
    pub fn new(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            root: None,
        }
    }

    /// The working directory to use for new terminals: the configured root, or the user's home
    /// directory if none is set.
    pub fn effective_root(&self) -> PathBuf {
        self.root
            .clone()
            .unwrap_or_else(|| home_dir())
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"))
}

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
    pub title: String,
    pub session_id: Option<SessionId>,
}

impl TabItem {
    pub fn terminal(session_id: SessionId, title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: ItemKind::Terminal,
            title: title.into(),
            session_id: Some(session_id),
        }
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
                id,
                ratio: current,
                ..
            } if *id == split_id => {
                *current = ratio;
                true
            }
            Self::Split { first, second, .. } => {
                first.set_split_ratio(split_id, ratio)
                    || second.set_split_ratio(split_id, ratio)
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

    /// Total number of tab items across all groups in this layout.
    pub fn item_count(&self) -> usize {
        match self {
            Self::Group { group } => group.items.len(),
            Self::Split { first, second, .. } => first.item_count() + second.item_count(),
        }
    }

    /// Iterate over all terminal items across all groups.
    pub fn terminal_items(&self) -> impl Iterator<Item = &TabItem> {
        let mut items = Vec::new();
        self.collect_terminal_items(&mut items);
        items.into_iter()
    }

    fn collect_terminal_items<'a>(&'a self, items: &mut Vec<&'a TabItem>) {
        match self {
            Self::Group { group } => {
                for item in &group.items {
                    if matches!(item.kind, ItemKind::Terminal) {
                        items.push(item);
                    }
                }
            }
            Self::Split { first, second, .. } => {
                first.collect_terminal_items(items);
                second.collect_terminal_items(items);
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoCloseExitedTerminal {
    Never,
    #[default]
    OnSuccess,
    Always,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub auto_close_exited_terminal: AutoCloseExitedTerminal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_own_independent_tab_lists_after_split() {
        let first = TabItem::terminal(Uuid::new_v4(), "shell 1");
        let mut layout = LayoutNode::group(first);
        let original_group = match &layout {
            LayoutNode::Group { group } => group.id,
            _ => unreachable!(),
        };
        let second = TabItem::terminal(Uuid::new_v4(), "shell 2");
        let second_group = layout
            .split_group(original_group, SplitAxis::Horizontal, second)
            .expect("group should split");

        assert_eq!(
            layout.find_group_mut(original_group).unwrap().items.len(),
            1
        );
        assert_eq!(layout.find_group_mut(second_group).unwrap().items.len(), 1);
    }

    #[test]
    fn terminal_items_can_be_found_across_split_groups() {
        let first = TabItem::terminal(Uuid::new_v4(), "shell 1");
        let mut layout = LayoutNode::group(first);
        let original_group = layout.first_group_id();
        let session_id = Uuid::new_v4();
        let second = TabItem::terminal(session_id, "shell 2");
        let second_item_id = second.id;
        let second_group = layout
            .split_group(original_group, SplitAxis::Horizontal, second)
            .unwrap();

        assert_eq!(
            layout.find_terminal_item(session_id),
            Some((second_group, second_item_id))
        );
    }

    #[test]
    fn terminal_titles_update_across_split_groups_by_session_id() {
        let first_session_id = Uuid::new_v4();
        let second_session_id = Uuid::new_v4();
        let mut layout = LayoutNode::group(TabItem::terminal(first_session_id, "shell 1"));
        let first_group_id = layout.first_group_id();
        let second_group_id = layout
            .split_group(
                first_group_id,
                SplitAxis::Horizontal,
                TabItem::terminal(second_session_id, "shell 2"),
            )
            .unwrap();

        assert!(layout.update_terminal_title(second_session_id, "OSC title"));
        assert!(!layout.update_terminal_title(second_session_id, "OSC title"));
        assert_eq!(
            layout.find_group(first_group_id).unwrap().items[0].title,
            "shell 1"
        );
        assert_eq!(
            layout.find_group(second_group_id).unwrap().items[0].title,
            "OSC title"
        );
    }

    #[test]
    fn closing_the_active_item_activates_a_neighbor() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let first_id = first.id;
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_id = second.id;
        let mut group = TabGroup::new(first);
        group.add_item(second);

        group.close_item(second_id);

        assert_eq!(group.active_item_id, Some(first_id));
    }

    #[test]
    fn closing_the_last_item_in_a_split_collapses_its_group() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let mut layout = LayoutNode::group(first);
        let original_group = layout.first_group_id();
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_item_id = second.id;
        let second_group = layout
            .split_group(original_group, SplitAxis::Vertical, second)
            .unwrap();

        let removed = layout.close_item(second_group, second_item_id).unwrap();

        assert_eq!(removed.title, "two");
        assert!(layout.find_group(second_group).is_none());
        assert_eq!(layout.first_group_id(), original_group);
    }

    #[test]
    fn closing_the_only_root_item_keeps_an_empty_group_for_new_tabs() {
        let only = TabItem::terminal(Uuid::new_v4(), "one");
        let item_id = only.id;
        let mut layout = LayoutNode::group(only);
        let group_id = layout.first_group_id();

        layout.close_item(group_id, item_id).unwrap();

        let group = layout.find_group(group_id).unwrap();
        assert!(group.items.is_empty());
        assert_eq!(group.active_item_id, None);
    }

    #[test]
    fn directional_splits_place_groups_on_the_requested_side() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let mut layout = LayoutNode::group(first);
        let original_group = layout.first_group_id();
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let new_group = layout
            .split_group_direction(original_group, Direction::Left, second)
            .unwrap();

        assert_eq!(layout.top_left_group_id(), new_group);
        assert_eq!(layout.top_right_group_id(), original_group);
        assert_eq!(
            layout.neighbor_group_id(new_group, Direction::Right),
            Some(original_group)
        );
        assert_eq!(
            layout.neighbor_group_id(original_group, Direction::Left),
            Some(new_group)
        );
    }

    #[test]
    fn moving_a_tab_to_another_group_prunes_an_empty_source_group() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let first_item_id = first.id;
        let mut layout = LayoutNode::group(first);
        let first_group = layout.first_group_id();
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_group = layout
            .split_group_direction(first_group, Direction::Right, second)
            .unwrap();

        assert_eq!(
            layout.move_item(first_group, first_item_id, second_group),
            Some(second_group)
        );
        assert!(layout.find_group(first_group).is_none());
        assert_eq!(layout.find_group(second_group).unwrap().items.len(), 2);
    }

    #[test]
    fn moving_a_tab_at_an_index_reorders_within_its_group() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let first_item_id = first.id;
        let mut layout = LayoutNode::group(first);
        let group_id = layout.first_group_id();
        let group = layout.find_group_mut(group_id).unwrap();
        group.add_item(TabItem::terminal(Uuid::new_v4(), "two"));
        group.add_item(TabItem::terminal(Uuid::new_v4(), "three"));

        assert_eq!(
            layout.move_item_at(group_id, first_item_id, group_id, 3),
            Some(group_id)
        );

        let group = layout.find_group(group_id).unwrap();
        assert_eq!(
            group
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["two", "three", "one"]
        );
        assert_eq!(group.active_item_id, Some(first_item_id));

        assert_eq!(
            layout.move_item_at(group_id, first_item_id, group_id, 0),
            Some(group_id)
        );
        assert_eq!(
            layout
                .find_group(group_id)
                .unwrap()
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["one", "two", "three"]
        );
    }

    #[test]
    fn moving_a_tab_at_an_index_inserts_into_another_group() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let first_item_id = first.id;
        let mut layout = LayoutNode::group(first);
        let first_group = layout.first_group_id();
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_group = layout
            .split_group_direction(first_group, Direction::Right, second)
            .unwrap();
        layout
            .find_group_mut(second_group)
            .unwrap()
            .add_item(TabItem::terminal(Uuid::new_v4(), "three"));

        assert_eq!(
            layout.move_item_at(first_group, first_item_id, second_group, 1),
            Some(second_group)
        );

        assert!(layout.find_group(first_group).is_none());
        let group = layout.find_group(second_group).unwrap();
        assert_eq!(
            group
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["two", "one", "three"]
        );
    }

    #[test]
    fn moving_a_tab_to_an_edge_creates_a_group_at_that_edge() {
        let first = TabItem::terminal(Uuid::new_v4(), "one");
        let second = TabItem::terminal(Uuid::new_v4(), "two");
        let second_item_id = second.id;
        let mut layout = LayoutNode::group(first);
        let original_group = layout.first_group_id();
        layout
            .find_group_mut(original_group)
            .unwrap()
            .add_item(second);

        let new_group = layout
            .move_item_to_new_group(
                original_group,
                second_item_id,
                original_group,
                Direction::Up,
            )
            .unwrap();

        assert_eq!(layout.top_left_group_id(), new_group);
        assert_eq!(layout.find_group(new_group).unwrap().items.len(), 1);
        assert_eq!(layout.find_group(original_group).unwrap().items.len(), 1);
    }

    #[test]
    fn a_sole_tab_cannot_split_its_own_group_by_moving_it() {
        let only = TabItem::terminal(Uuid::new_v4(), "one");
        let item_id = only.id;
        let mut layout = LayoutNode::group(only);
        let group_id = layout.first_group_id();

        assert_eq!(
            layout.move_item_to_new_group(group_id, item_id, group_id, Direction::Down),
            None
        );
        assert_eq!(layout.find_group(group_id).unwrap().items.len(), 1);
    }

    #[test]
    fn nested_splits_have_distinct_ids_and_resize_independently() {
        // Build: root Horizontal split, whose `first` child is itself a Vertical split. The inner
        // split's first group is also the root's first group, so identifying a split by its first
        // group would make the two collide. Each must instead have its own id.
        let top_left = TabItem::terminal(Uuid::new_v4(), "top-left");
        let mut layout = LayoutNode::group(top_left);
        let left_group = layout.first_group_id();

        // Split to the right first -> outer Horizontal split (left column / right).
        layout
            .split_group_direction(
                left_group,
                Direction::Right,
                TabItem::terminal(Uuid::new_v4(), "right"),
            )
            .unwrap();
        // Then split the left group downward -> inner Vertical split nested in the outer's `first`.
        layout
            .split_group_direction(
                left_group,
                Direction::Down,
                TabItem::terminal(Uuid::new_v4(), "bottom-left"),
            )
            .unwrap();

        let (outer_id, inner_id) = match &layout {
            LayoutNode::Split {
                id: outer_id,
                axis: SplitAxis::Horizontal,
                first,
                ..
            } => match first.as_ref() {
                LayoutNode::Split {
                    id: inner_id,
                    axis: SplitAxis::Vertical,
                    ..
                } => (*outer_id, *inner_id),
                other => panic!("expected inner vertical split, got {other:?}"),
            },
            other => panic!("expected outer horizontal split, got {other:?}"),
        };

        assert_ne!(outer_id, inner_id, "nested splits must not share an id");
        assert_eq!(layout.split_axis(outer_id), Some(SplitAxis::Horizontal));
        assert_eq!(layout.split_axis(inner_id), Some(SplitAxis::Vertical));

        // Resizing the inner split must not touch the outer split's ratio, and vice versa.
        assert!(layout.set_split_ratio(inner_id, 0.3));
        assert_eq!(layout.split_ratio(inner_id), Some(0.3));
        assert_eq!(layout.split_ratio(outer_id), Some(0.5));

        assert!(layout.set_split_ratio(outer_id, 0.7));
        assert_eq!(layout.split_ratio(outer_id), Some(0.7));
        assert_eq!(layout.split_ratio(inner_id), Some(0.3));
    }
}
