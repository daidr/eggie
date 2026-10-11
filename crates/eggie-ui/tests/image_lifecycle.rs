use gpui::{Context, Render, RenderImage, TestAppContext, Window, div, img, prelude::*, px};
use image::{Frame, RgbaImage};
use std::{cell::Cell, rc::Rc, sync::Arc};

fn image() -> Arc<RenderImage> {
    Arc::new(RenderImage::new([Frame::new(RgbaImage::new(256, 160))]))
}

struct ImageView {
    image: Option<Arc<RenderImage>>,
}

impl Render for ImageView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().children(
            self.image
                .clone()
                .map(|image| img(image).w(px(256.)).h(px(160.))),
        )
    }
}

#[gpui::test]
fn replacing_animation_frames_does_not_accumulate_gpu_images(cx: &mut TestAppContext) {
    let window = cx.add_window(|_, _| ImageView { image: None });
    let mut ids = Vec::new();
    for _ in 0..300 {
        let frame = image();
        ids.push(frame.id);
        cx.update_window(window.into(), |view, window, cx| {
            let view = view.downcast::<ImageView>().unwrap();
            view.update(cx, |view, cx| {
                view.image = Some(frame);
                cx.notify();
            });
            window.draw(cx).clear(cx);
            assert_eq!(window.image_atlas_source_count(), 1);
        })
        .unwrap();
    }
    cx.update_window(window.into(), |_, window, _| {
        for (index, id) in ids.iter().copied().enumerate() {
            let mut probe = RenderImage::new([Frame::new(RgbaImage::new(1, 1))]);
            probe.id = id;
            assert_eq!(window.has_image_atlas_entry(&probe), index == ids.len() - 1);
        }
    })
    .unwrap();
    cx.update_window(window.into(), |view, window, cx| {
        let view = view.downcast::<ImageView>().unwrap();
        view.update(cx, |view, cx| {
            view.image = None;
            cx.notify();
        });
        window.draw(cx).clear(cx);
        assert_eq!(window.image_atlas_source_count(), 0);
    })
    .unwrap();
}

#[gpui::test]
fn shared_images_survive_until_all_windows_and_callers_release_them(cx: &mut TestAppContext) {
    let image = image();
    let a = cx.add_window(|_, _| ImageView {
        image: Some(image.clone()),
    });
    let b = cx.add_window(|_, _| ImageView {
        image: Some(image.clone()),
    });
    for window in [a, b] {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert!(window.has_image_atlas_entry(&image));
        })
        .unwrap();
    }
    cx.update_window(a.into(), |view, window, cx| {
        let view = view.downcast::<ImageView>().unwrap();
        view.update(cx, |view, cx| {
            view.image = None;
            cx.notify();
        });
        window.draw(cx).clear(cx);
        assert!(window.has_image_atlas_entry(&image));
    })
    .unwrap();
    drop(image);
    cx.update_window(b.into(), |view, window, cx| {
        let view = view.downcast::<ImageView>().unwrap();
        view.update(cx, |view, cx| {
            view.image = None;
            cx.notify();
        });
        window.draw(cx).clear(cx);
        assert_eq!(window.image_atlas_source_count(), 0);
    })
    .unwrap();
    cx.update_window(a.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        assert_eq!(window.image_atlas_source_count(), 0);
    })
    .unwrap();
}

struct CachedImage {
    renders: Rc<Cell<usize>>,
}

impl Render for CachedImage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        img(image()).w(px(256.)).h(px(160.))
    }
}

#[gpui::test]
fn cached_view_replays_keep_their_image_alive(cx: &mut TestAppContext) {
    let renders = Rc::new(Cell::new(0));
    let window = cx.add_window(|_, cx| {
        let child = cx.new(|_| CachedImage {
            renders: renders.clone(),
        });
        // Rendering the child produces a temporary image with no application cache owner.
        struct Parent(gpui::Entity<CachedImage>);
        impl Render for Parent {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let mut style = div().w(px(256.)).h(px(160.));
                div().child(self.0.clone().cached(style.style().clone()))
            }
        }
        Parent(child)
    });
    for _ in 0..10 {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(window.image_atlas_source_count(), 1);
        })
        .unwrap();
    }
    assert_eq!(
        renders.get(),
        1,
        "the child must actually replay cached paint"
    );
}
