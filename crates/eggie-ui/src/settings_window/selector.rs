use super::*;

impl SettingsWindow {
    pub(super) fn render_selector(
        &self,
        id: &'static str,
        value: String,
        font_family: Option<String>,
        kind: SelectorKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact = kind == SelectorKind::SshHost;
        let managed = self.lua_manages_selector(kind, cx);
        let dropdown_position = self.selector_dropdown_position(kind, window);
        let selector = cx.entity().downgrade();
        div()
            .on_children_prepainted(move |children, _, cx| {
                let Some(bounds) = children.first().copied() else {
                    return;
                };
                let selector = selector.clone();
                cx.defer(move |cx| {
                    selector
                        .update(cx, |settings, cx| {
                            let stored = &mut settings.selector_bounds[kind.index()];
                            if *stored != Some(bounds) {
                                *stored = Some(bounds);
                                if settings.open_selector == Some(kind) {
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                });
            })
            .id(format!("{id}-container"))
            .relative()
            .w(px(if compact { 30. } else { SELECTOR_WIDTH }))
            .h(px(if compact { 28. } else { 32. }))
            .child(
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .size_full()
                    .when(!compact, |element| {
                        element
                            .rounded_lg()
                            .px_3()
                            .border_1()
                            .bg(rgb(self.colors.panel_alt))
                    })
                    .when(compact, |element| {
                        element
                            .justify_center()
                            .border_l_1()
                            .tooltip(tooltip_text(SharedString::from(value.clone()), self.colors))
                    })
                    .border_color(rgb(self.colors.border))
                    .when(!managed, |element| {
                        element
                            .cursor_pointer()
                            .hover(|element| element.border_color(rgb(self.colors.accent)))
                    })
                    .on_click(cx.listener(move |settings, _, window, cx| {
                        settings.toggle_selector(kind, window, cx)
                    }))
                    .when(!compact, |element| {
                        element.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when_some(font_family, |element, family| {
                                    element.font_family(SharedString::from(family))
                                })
                                .child(if managed {
                                    format!("{value} · init.lua")
                                } else {
                                    value
                                }),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(self.colors.muted))
                            .child(icon(IconName::ArrowDown)),
                    ),
            )
            .when(self.open_selector == Some(kind), |selector| {
                let mut dropdown = anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(SELECTOR_WINDOW_MARGIN));
                if let Some(position) = dropdown_position {
                    dropdown = dropdown.position(position);
                }
                selector.child(
                    deferred(dropdown.child(self.render_selector_dropdown(kind, window, cx)))
                        .priority(2),
                )
            })
            .into_any_element()
    }

    pub(super) fn toggle_selector(
        &mut self,
        kind: SelectorKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.lua_manages_selector(kind, cx) {
            return;
        }
        if self.open_selector == Some(kind) {
            self.open_selector = None;
            self.clear_theme_preview(cx);
        } else {
            self.open_selector = Some(kind);
            if kind == SelectorKind::SshHost {
                self.load_ssh_hosts(cx);
            }
            let language = self.settings.read(cx).config().language;
            let placeholder = match kind {
                SelectorKind::SshHost => {
                    if language == Language::SimplifiedChinese {
                        "搜索 SSH Host"
                    } else {
                        "Search SSH hosts"
                    }
                }
                // Every theme slot searches one half of the catalog, so the placeholder
                // follows the slot's darkness rather than naming the layer.
                SelectorKind::InterfaceDark
                | SelectorKind::InterfaceLight
                | SelectorKind::TerminalDark
                | SelectorKind::TerminalLight
                | SelectorKind::ProfileTerminalDark
                | SelectorKind::ProfileTerminalLight => {
                    if kind.is_dark() {
                        language.search_dark_themes()
                    } else {
                        language.search_light_themes()
                    }
                }
                SelectorKind::Font
                | SelectorKind::FontBold
                | SelectorKind::FontItalic
                | SelectorKind::FontBoldItalic
                | SelectorKind::ProfileFont => language.search_fonts(),
            };
            self.selector_search_input.update(cx, |input, cx| {
                input.set_placeholder(placeholder);
                input.set_content("", cx);
            });
            self.selector_scroll_handle
                .set_offset(point(px(0.), px(0.)));
            let handle = self.selector_search_input.read(cx).focus_handle();
            handle.focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn selector_dropdown_position(
        &self,
        kind: SelectorKind,
        window: &Window,
    ) -> Option<gpui::Point<Pixels>> {
        self.selector_bounds[kind.index()].map(|bounds| {
            let dropdown_height = px(SELECTOR_DROPDOWN_HEIGHT);
            let gap = px(SELECTOR_DROPDOWN_GAP);
            let margin = px(SELECTOR_WINDOW_MARGIN);
            let y = if window.viewport_size().height - bounds.bottom()
                >= dropdown_height + gap + margin
            {
                bounds.bottom() + gap
            } else {
                bounds.top() - dropdown_height - gap
            };
            let x = if kind == SelectorKind::SshHost {
                bounds.right() - px(SELECTOR_WIDTH)
            } else {
                bounds.left()
            };
            point(x, y)
        })
    }

    pub(super) fn render_selector_dropdown(
        &self,
        kind: SelectorKind,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config = self.settings.read(cx).config();
        let language = config.language;
        // The option list comes from `selector_names` (the single source), so this only has to answer
        // "what is the current value". Exhaustive on purpose: a catch-all would let a new slot read
        // another slot's field, which compiles and silently shows the wrong selection.
        let current = match kind {
            SelectorKind::SshHost => self
                .profile_inputs
                .remote_host
                .read(cx)
                .content()
                .to_owned(),
            SelectorKind::InterfaceDark => config.interface_dark_theme.clone(),
            SelectorKind::InterfaceLight => config.interface_light_theme.clone(),
            SelectorKind::TerminalDark => config.terminal_dark_theme.clone(),
            SelectorKind::TerminalLight => config.terminal_light_theme.clone(),
            SelectorKind::Font => config.font_family.clone(),
            SelectorKind::FontBold => config.font_family_bold.clone(),
            SelectorKind::FontItalic => config.font_family_italic.clone(),
            SelectorKind::FontBoldItalic => config.font_family_bold_italic.clone(),
            // Profile overrides read from the store, not from `config`. An unset override is an empty
            // string here, which is what makes the "follow global setting" entry render as selected.
            SelectorKind::ProfileTerminalDark
            | SelectorKind::ProfileTerminalLight
            | SelectorKind::ProfileFont => {
                let id = self.selected_profile_id(cx);
                let appearance = self
                    .profile_store
                    .read(cx)
                    .profile(id)
                    .map(|profile| profile.appearance.clone())
                    .unwrap_or_default();
                match kind {
                    SelectorKind::ProfileTerminalDark => {
                        appearance.terminal_dark_theme.unwrap_or_default()
                    }
                    SelectorKind::ProfileTerminalLight => {
                        appearance.terminal_light_theme.unwrap_or_default()
                    }
                    _ => appearance.font_family.unwrap_or_default(),
                }
            }
        };
        let items = self.selector_names(kind).to_vec();
        let query_owned = self
            .selector_search_input
            .read(cx)
            .content()
            .trim()
            .to_owned();
        let items = items
            .into_iter()
            .filter(|item| selector_matches(item, &query_owned))
            .collect::<Vec<_>>();
        let has_items = !items.is_empty();

        // Per-style families (bold/italic/bold-italic) can be cleared back to "follow the regular
        // family", and every profile override back to "follow the global setting". Offer that as a
        // pinned first entry when not actively searching, so a set value is always reversible (an
        // empty stored value means "inherit"). The global regular Font and the global themes always
        // resolve to a concrete value, so they get no clear entry.
        let clearable = matches!(
            kind,
            SelectorKind::FontBold | SelectorKind::FontItalic | SelectorKind::FontBoldItalic
        ) || kind.is_profile();
        let show_clear = clearable && query_owned.is_empty();
        let clear_selected = show_clear && current.trim().is_empty();
        // The two clear entries say different things: a style font falls back to the regular family,
        // a profile override falls back to the global setting.
        let clear_label = if kind.is_profile() {
            language.profile_appearance_inherit()
        } else {
            language.font_family_use_regular()
        };

        let mut list = div()
            .id("settings-selector-list")
            .flex()
            .flex_1()
            .min_h_0()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.selector_scroll_handle)
            .py_1();
        if show_clear {
            list = list.child(
                div()
                    .id("settings-selector-clear")
                    .flex()
                    .items_center()
                    .min_w_0()
                    .flex_none()
                    .h(px(32.))
                    .mx_1()
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .when(clear_selected, |element| {
                        element
                            .bg(rgb(self.colors.panel_alt))
                            .text_color(rgb(self.colors.accent))
                    })
                    .when(!clear_selected, |element| {
                        element.text_color(rgb(self.colors.muted))
                    })
                    .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    .on_click(cx.listener(move |settings, _, _, cx| {
                        if kind.is_profile() {
                            settings.apply_profile_selector_choice(kind, None, cx);
                        } else {
                            settings.settings.update(cx, |store, cx| {
                                store.update(
                                    // Exhaustive on purpose: the non-clearable slots have to be listed
                                    // rather than swept into a catch-all, or a new clearable slot would
                                    // compile with a clear button that silently does nothing.
                                    |config| match kind {
                                        SelectorKind::FontBold => config.font_family_bold.clear(),
                                        SelectorKind::FontItalic => {
                                            config.font_family_italic.clear()
                                        }
                                        SelectorKind::FontBoldItalic => {
                                            config.font_family_bold_italic.clear()
                                        }
                                        // Not clearable: these always resolve to a concrete value, so
                                        // `clearable` never offers this row for them.
                                        SelectorKind::InterfaceDark
                                        | SelectorKind::InterfaceLight
                                        | SelectorKind::TerminalDark
                                        | SelectorKind::TerminalLight
                                        | SelectorKind::Font => {}
                                        // Handled above: these write to the profile store.
                                        SelectorKind::ProfileTerminalDark
                                        | SelectorKind::ProfileTerminalLight
                                        | SelectorKind::ProfileFont
                                        | SelectorKind::SshHost => {}
                                    },
                                    cx,
                                )
                            });
                        }
                        settings.open_selector = None;
                        cx.notify();
                    }))
                    .child(div().min_w_0().truncate().child(clear_label)),
            );
        }
        // Theme selectors preview their layer across the interface and real terminals.
        // Font selectors don't preview live.
        // Every theme slot previews — including the per-profile overrides, which reach the same live
        // path but apply only to that profile's terminals.
        let is_theme_selector = kind.is_theme();
        for (index, item) in items.into_iter().enumerate() {
            let selected = item == current;
            let selected_item = item.clone();
            let hover_item = item.clone();
            list = list.child(
                div()
                    .id(("settings-selector-option", index))
                    .flex()
                    .items_center()
                    .min_w_0()
                    .flex_none()
                    .h(px(32.))
                    .mx_1()
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |element| {
                        element
                            .bg(rgb(self.colors.panel_alt))
                            .text_color(rgb(self.colors.accent))
                    })
                    .when(kind.is_font(), |element| {
                        element.font_family(SharedString::from(item.clone()))
                    })
                    .hover(|element| element.bg(rgb(self.colors.panel_alt)))
                    .when(is_theme_selector, |element| {
                        element.on_hover(cx.listener(move |settings, hovered: &bool, _, cx| {
                            // Preview on enter only; leaving an option to enter the next one must not
                            // flip back to the persisted theme mid-glide. The dropdown container's
                            // own on_hover clears the preview once the pointer leaves the list.
                            if *hovered {
                                settings.preview_theme(kind, &hover_item, cx);
                            }
                        }))
                    })
                    .on_click(cx.listener(move |settings, _, window, cx| {
                        // Committing supersedes the preview: clear it so the resolved theme falls back
                        // to the now-persisted value (identical pixels, but no stale override left).
                        settings.clear_theme_preview(cx);
                        settings.commit_selector_choice(kind, selected_item.clone(), cx);
                        settings.open_selector = None;
                        if kind == SelectorKind::SshHost {
                            settings
                                .profile_inputs
                                .remote_host
                                .read(cx)
                                .focus_handle()
                                .focus(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(div().min_w_0().truncate().child(item)),
            );
        }
        if !has_items && !show_clear {
            list = list.child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(self.colors.muted))
                    .child(
                        if kind == SelectorKind::SshHost && self.ssh_hosts_task.is_some() {
                            if language == Language::SimplifiedChinese {
                                "读取中"
                            } else {
                                "Loading"
                            }
                        } else if kind == SelectorKind::SshHost && self.ssh_hosts_error.is_some() {
                            if language == Language::SimplifiedChinese {
                                "无法读取 SSH 配置"
                            } else {
                                "Could not read SSH config"
                            }
                        } else if kind == SelectorKind::SshHost && self.ssh_host_names.is_empty() {
                            if language == Language::SimplifiedChinese {
                                "未找到 SSH 主机"
                            } else {
                                "No SSH hosts found"
                            }
                        } else {
                            language.no_matches()
                        },
                    ),
            );
        }

        let search = div()
            .id("settings-selector-search")
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(30.))
            .px_3()
            // Flush against the dropdown's edges: no outer margin/border/radius, just a divider that
            // separates the search field from the results list below.
            .border_b_1()
            .border_color(rgb(self.colors.border))
            .text_size(px(12.))
            .child(
                div()
                    .flex_none()
                    .text_color(rgb(self.colors.muted))
                    .child(icon_sized(IconName::Search, 13.)),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .text_color(rgb(self.colors.text))
                    .child(self.selector_search_input.clone()),
            );

        div()
            .id("settings-selector-dropdown")
            .flex()
            .flex_col()
            .w(px(SELECTOR_WIDTH))
            .h(px(SELECTOR_DROPDOWN_HEIGHT))
            .overflow_hidden()
            .rounded_xl()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.panel))
            .shadow_lg()
            // Floating overlay: block every mouse event (and hover/tooltip) for the settings rows
            // painted behind it, so clicks on the dropdown's blank areas (search padding, list gaps)
            // can't bubble through and misfire a control underneath. This is the canonical GPUI way —
            // one call replaces per-event stop_propagation patches. Children paint on top, so the
            // search input and list items still receive their own events.
            .occlude()
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .when(is_theme_selector, |element| {
                // Pointer left the dropdown entirely (or entered its blank gaps between options):
                // drop the hover preview so the persisted theme comes back. Option-to-option glides
                // stay previewed because each option re-arms the preview on enter.
                element.on_hover(cx.listener(move |settings, hovered: &bool, _, cx| {
                    if !*hovered {
                        settings.clear_theme_preview(cx);
                    }
                }))
            })
            .on_mouse_down_out(cx.listener(move |_, _, window, cx| {
                cx.defer_in(window, move |settings, _, cx| {
                    if settings.open_selector == Some(kind) {
                        settings.open_selector = None;
                        settings.clear_theme_preview(cx);
                        cx.notify();
                    }
                });
            }))
            .child(search)
            .child(list)
            .into_any_element()
    }

    // --- Keyboard shortcut recording ---------------------------------------------------------
}
