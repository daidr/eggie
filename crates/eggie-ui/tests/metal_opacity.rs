#![cfg(target_os = "macos")]

use gpui::{Context, Render, TestAppContext, Window, canvas, div, prelude::*};
use std::sync::Arc;

struct Noop;
impl gpui::MetalPrimitiveRenderer for Noop {
    fn paint(&self, _: &mut gpui::MetalRenderContext<'_>) {}
}

struct View;
impl Render for View {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().opacity(0.5).child(
            div().size_full().opacity(0.4).child(
                canvas(
                    |_, _, _| (),
                    |bounds, _, window, _| window.paint_metal(bounds, Arc::new(Noop)),
                )
                .size_full(),
            ),
        )
    }
}

#[gpui::test]
fn custom_metal_inherits_nested_element_opacity(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| View);
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let primitives = &window.rendered_scene().metal_primitives;
        assert_eq!(primitives.len(), 1);
        assert!((primitives[0].opacity - 0.2).abs() < f32::EPSILON);
    })
    .unwrap();
}
