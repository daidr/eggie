//! Sidebar project drag presentation and insertion geometry; ordering lives in ProjectStore.

use std::sync::Arc;

use eggie_domain::{ProjectId, WindowId};
use gpui::{
    Bounds, Context, Image, IntoElement, Pixels, Point, Render, ScrollHandle, SharedString, Window,
    div, img, prelude::*, px, rgb,
};

use crate::settings::UiColors;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProjectDropTarget {
    pub(crate) before: Option<ProjectId>,
}

#[derive(Default)]
pub(crate) struct ProjectDragState {
    pub(crate) source: Option<ProjectId>,
    pub(crate) target: Option<ProjectDropTarget>,
    pub(crate) bounds: Option<Bounds<Pixels>>,
    pub(crate) scroll: ScrollHandle,
    pub(crate) revision: u64,
}

/// Use header midpoints, so hovering an expanded project's terminals inserts after that project.
pub(crate) fn insertion_target(
    rows: impl Iterator<Item = (ProjectId, Bounds<Pixels>)>,
    pointer_y: Pixels,
) -> ProjectDropTarget {
    ProjectDropTarget {
        before: rows
            .filter(|(_, bounds)| pointer_y < bounds.center().y)
            .map(|(id, _)| id)
            .next(),
    }
}

/// Ease into edge scrolling; moving outside the list stops it immediately.
pub(crate) fn edge_scroll_delta(bounds: Bounds<Pixels>, pointer: Point<Pixels>) -> Pixels {
    if !bounds.contains(&pointer) {
        return px(0.);
    }
    let edge = 32_f32.min(f32::from(bounds.size.height) / 2.);
    if edge <= 0. {
        return px(0.);
    }
    let top = (1. - f32::from(pointer.y - bounds.top()) / edge).clamp(0., 1.);
    let bottom = (1. - f32::from(bounds.bottom() - pointer.y) / edge).clamp(0., 1.);
    px(7. * (top * top - bottom * bottom))
}

#[derive(Clone)]
pub(crate) struct DraggedProject {
    pub(crate) id: ProjectId,
    pub(crate) window_id: WindowId,
    pub(crate) title: SharedString,
    pub(crate) icon: Arc<Image>,
    pub(crate) icon_width: f32,
    pub(crate) icon_height: f32,
    pub(crate) width: f32,
    pub(crate) colors: UiColors,
}

impl Render for DraggedProject {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .w(px(self.width))
            .py_1()
            .px_2()
            .gap_3()
            .rounded_md()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel_alt))
            .text_color(rgb(self.colors.text))
            .text_sm()
            .shadow_lg()
            .opacity(0.9)
            .child(
                img(self.icon.clone())
                    .flex_none()
                    .w(px(self.icon_width))
                    .h(px(self.icon_height)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.title.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    #[test]
    fn expanded_projects_keep_their_terminals_together() {
        let a = ProjectId::new_v4();
        let b = ProjectId::new_v4();
        let rows = [
            (
                a,
                Bounds::new(point(px(0.), px(10.)), size(px(200.), px(32.))),
            ),
            // The first project has expanded terminals between its header and the next header.
            (
                b,
                Bounds::new(point(px(0.), px(150.)), size(px(200.), px(32.))),
            ),
        ];
        for (y, before) in [(10., Some(a)), (26., Some(b)), (90., Some(b)), (166., None)] {
            assert_eq!(insertion_target(rows.into_iter(), px(y)).before, before);
        }
    }

    #[test]
    fn edge_scrolling_is_directional_and_stops_outside_the_viewport() {
        let bounds = Bounds::new(point(px(10.), px(20.)), gpui::size(px(200.), px(300.)));
        let delta = |x, y| edge_scroll_delta(bounds, point(px(x), px(y)));
        assert!(delta(50., 21.) > delta(50., 40.));
        assert!(delta(50., 40.) > px(0.));
        assert_eq!(delta(50., 100.), px(0.));
        assert!(delta(50., 319.) < px(0.));
        assert_eq!(delta(0., 21.), px(0.));
        assert_eq!(delta(50., 321.), px(0.));
    }
}
