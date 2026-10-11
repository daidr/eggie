use super::*;

impl SettingsWindow {
    pub(super) fn render_about_section(
        &self,
        language: Language,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = self.colors;
        // Copy the two update-preference values out so the `settings` borrow is
        // released before the `&mut cx` render calls below.
        let (auto_check_updates, update_channel) = {
            let config = self.settings.read(cx).config();
            (config.auto_check_updates, config.update_channel)
        };
        let auto_check_updates_control =
            self.render_auto_check_updates_control(auto_check_updates, cx);
        let update_channel_control = self.render_update_channel_control(update_channel, cx);

        let build_mode = if cfg!(debug_assertions) {
            language.build_mode_debug()
        } else {
            language.build_mode_release()
        };

        // A metadata line: label on the left, value right-aligned and muted.
        let metadata_row = |label: &'static str, value: String| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_6()
                .min_h(px(44.))
                .px_4()
                .py_2()
                .child(div().flex_none().text_color(rgb(colors.text)).child(label))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(rgb(colors.muted))
                        .text_right()
                        .child(value),
                )
                .into_any_element()
        };

        let header = div()
            .flex()
            .flex_none()
            .flex_col()
            .items_center()
            .gap_2()
            .py_4()
            .child(img(logo_image()).w(px(72.)).h(px(72.)))
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(colors.text))
                    .child("Eggie"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(colors.muted))
                    .child(format!(
                        "{} {}",
                        language.version_row(),
                        eggie_build_info::VERSION
                    )),
            )
            .child(div().pt_2().child(crate::update_window::button(
                language.check_for_updates_button(),
                true,
                colors,
                cx.listener(|window, _, _, cx| {
                    let channel = window.settings.read(cx).config().update_channel.slug();
                    window.updates.update(cx, |controller, cx| {
                        if matches!(
                            controller.state(),
                            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Error(_)
                        ) {
                            controller.check(false, channel, cx);
                        }
                    });
                    crate::update_window::open_update_window(
                        window.updates.clone(),
                        window.settings.clone(),
                        cx,
                    );
                }),
            )))
            .into_any_element();

        let metadata = settings_section(
            Some(language.build_info_section()),
            vec![
                metadata_row(language.version_row(), eggie_build_info::VERSION.to_owned()),
                metadata_row(
                    language.protocol_version_row(),
                    eggie_protocol::PROTOCOL_VERSION.to_string(),
                ),
                metadata_row(
                    language.build_date_row(),
                    eggie_build_info::BUILD_DATE.to_owned(),
                ),
                metadata_row(
                    language.build_id_row(),
                    eggie_build_info::BUILD_ID.to_owned(),
                ),
                metadata_row(language.build_mode_row(), build_mode.to_owned()),
            ],
            colors,
        );

        let updates = settings_section(
            Some(language.updates_section()),
            vec![
                settings_row(
                    language.auto_check_updates_row(),
                    Some(language.auto_check_updates_description()),
                    auto_check_updates_control,
                    colors,
                ),
                settings_row(
                    language.update_channel_row(),
                    Some(language.update_channel_description()),
                    update_channel_control,
                    colors,
                ),
            ],
            colors,
        );

        vec![header, metadata, updates]
    }
}
