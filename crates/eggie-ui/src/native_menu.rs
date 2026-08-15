use eggie_domain::Direction;
use gpui::Window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeTabMenuCommand {
    Split(Direction),
    Move(Direction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeProjectMenuCommand {
    EditName,
    SetRoot,
    CloseTabs,
    DeleteProject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeProcessMenuCommand {
    Terminate,
    ForceKill,
    CopyPid,
    CopyExecutablePath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeSnippetMenuCommand {
    Use,
    Edit,
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeEditMenuCommand {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[cfg(target_os = "macos")]
pub(crate) use macos::{
    NativeEditMenu, NativeProcessMenu, NativeProjectMenu, NativeSnippetMenu, NativeTabMenu,
};

#[cfg(target_os = "macos")]
pub(crate) fn prepare_tab_menu(
    window: &Window,
    move_enabled: [bool; 4],
    language: crate::settings::Language,
) -> Option<NativeTabMenu> {
    macos::NativeTabMenu::prepare(window, move_enabled, language)
}

#[cfg(target_os = "macos")]
pub(crate) fn prepare_project_menu(
    window: &Window,
    tab_count: usize,
    language: crate::settings::Language,
) -> Option<NativeProjectMenu> {
    macos::NativeProjectMenu::prepare(window, tab_count, language)
}

#[cfg(target_os = "macos")]
pub(crate) fn prepare_process_menu(
    window: &Window,
    language: crate::settings::Language,
) -> Option<NativeProcessMenu> {
    macos::NativeProcessMenu::prepare(window, language)
}

#[cfg(target_os = "macos")]
pub(crate) fn prepare_snippet_menu(
    window: &Window,
    language: crate::settings::Language,
) -> Option<NativeSnippetMenu> {
    macos::NativeSnippetMenu::prepare(window, language)
}

#[cfg(target_os = "macos")]
pub(crate) fn prepare_edit_menu(
    window: &Window,
    enabled: [bool; 4],
    language: crate::settings::Language,
) -> Option<NativeEditMenu> {
    macos::NativeEditMenu::prepare(window, enabled, language)
}

#[cfg(not(target_os = "macos"))]
pub(crate) struct NativeTabMenu;

#[cfg(not(target_os = "macos"))]
impl NativeTabMenu {
    pub(crate) fn show(self) -> Option<NativeTabMenuCommand> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn prepare_tab_menu(
    _: &Window,
    _: [bool; 4],
    _: crate::settings::Language,
) -> Option<NativeTabMenu> {
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) struct NativeProjectMenu;

#[cfg(not(target_os = "macos"))]
impl NativeProjectMenu {
    pub(crate) fn show(self) -> Option<NativeProjectMenuCommand> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn prepare_project_menu(
    _: &Window,
    _: usize,
    _: crate::settings::Language,
) -> Option<NativeProjectMenu> {
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) struct NativeProcessMenu;

#[cfg(not(target_os = "macos"))]
impl NativeProcessMenu {
    pub(crate) fn show(self) -> Option<NativeProcessMenuCommand> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn prepare_process_menu(_: &Window, _: crate::settings::Language) -> Option<NativeProcessMenu> {
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) struct NativeSnippetMenu;

#[cfg(not(target_os = "macos"))]
impl NativeSnippetMenu {
    pub(crate) fn show(self) -> Option<NativeSnippetMenuCommand> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn prepare_snippet_menu(_: &Window, _: crate::settings::Language) -> Option<NativeSnippetMenu> {
    None
}

#[cfg(not(target_os = "macos"))]
pub(crate) struct NativeEditMenu;

#[cfg(not(target_os = "macos"))]
impl NativeEditMenu {
    pub(crate) fn show(self) -> Option<NativeEditMenuCommand> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn prepare_edit_menu(
    _: &Window,
    _: [bool; 4],
    _: crate::settings::Language,
) -> Option<NativeEditMenu> {
    None
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{
        NativeEditMenuCommand, NativeProcessMenuCommand, NativeProjectMenuCommand,
        NativeSnippetMenuCommand, NativeTabMenuCommand,
    };
    use cocoa::{
        appkit::{NSApp, NSMenu, NSMenuItem},
        base::{NO, YES, id, nil},
        foundation::{NSAutoreleasePool, NSString, NSPoint},
    };
    use eggie_domain::Direction;
    use gpui::Window;
    use objc::{
        class,
        declare::ClassDecl,
        msg_send,
        runtime::{Class, Object, Sel},
        sel, sel_impl,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::OnceLock;

    const NO_SELECTION: isize = -1;
    const SELECTED_COMMAND_IVAR: &str = "_eggieSelectedCommand";
    static MENU_TARGET_CLASS: OnceLock<&'static Class> = OnceLock::new();

    pub(crate) struct NativeTabMenu {
        view: id,
        event: id,
        move_enabled: [bool; 4],
        language: crate::settings::Language,
    }

    impl NativeTabMenu {
        pub(super) fn prepare(
            window: &Window,
            move_enabled: [bool; 4],
            language: crate::settings::Language,
        ) -> Option<Self> {
            let window_handle = HasWindowHandle::window_handle(window).ok()?;
            let RawWindowHandle::AppKit(appkit_handle) = window_handle.as_raw() else {
                return None;
            };
            let view = appkit_handle.ns_view.as_ptr() as id;
            let event: id = unsafe { msg_send![NSApp(), currentEvent] };
            if view == nil || event == nil {
                return None;
            }
            unsafe {
                let _: id = msg_send![view, retain];
                let _: id = msg_send![event, retain];
            }
            Some(Self {
                view,
                event,
                move_enabled,
                language,
            })
        }

        /// Show the modal AppKit menu without holding a GPUI `App`/entity borrow.
        ///
        /// `NSMenu` runs a nested event loop. Calling this from a GPUI listener lets unrelated
        /// foreground tasks re-enter GPUI while its `RefCell` is already mutably borrowed.
        pub(crate) fn show(self) -> Option<NativeTabMenuCommand> {
            show_tab_menu(self.view, self.event, self.move_enabled, self.language)
        }
    }

    impl Drop for NativeTabMenu {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.event, release];
                let _: () = msg_send![self.view, release];
            }
        }
    }

    extern "C" fn select_menu_item(this: &Object, _: Sel, sender: id) {
        let tag: isize = unsafe { msg_send![sender, tag] };
        unsafe {
            let this = this as *const Object as *mut Object;
            (*this).set_ivar(SELECTED_COMMAND_IVAR, tag);
        }
    }

    fn menu_target_class() -> &'static Class {
        MENU_TARGET_CLASS.get_or_init(|| unsafe {
            let mut declaration = ClassDecl::new("EggieNativeMenuTarget", class!(NSObject))
                .expect("native menu target class must only be registered once");
            declaration.add_ivar::<isize>(SELECTED_COMMAND_IVAR);
            declaration.add_method(
                sel!(selectEggieTabMenuItem:),
                select_menu_item as extern "C" fn(&Object, Sel, id),
            );
            declaration.register()
        })
    }

    /// Show a menu at the event's mouse location.
    ///
    /// Uses `popUpMenuPositioningItem:atLocation:inView:` instead of
    /// `popUpContextMenu:withEvent:forView:`. The latter triggers macOS's
    /// contextual-menu pipeline which auto-appends an "AutoFill" submenu when
    /// the window's first responder handles text input (our terminal view).
    unsafe fn pop_up_menu(menu: id, event: id, view: id) {
        let window_point: NSPoint = msg_send![event, locationInWindow];
        let menu_point: NSPoint = msg_send![view, convertPoint: window_point fromView: nil];
        let _: bool = msg_send![menu, popUpMenuPositioningItem: nil atLocation: menu_point inView: view];
    }

    fn show_tab_menu(
        view: id,
        event: id,
        move_enabled: [bool; 4],
        language: crate::settings::Language,
    ) -> Option<NativeTabMenuCommand> {
        let selected = unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let target: id = msg_send![menu_target_class(), new];
            (*target).set_ivar(SELECTED_COMMAND_IVAR, NO_SELECTION);
            let menu = NSMenu::new(nil).autorelease();
            menu.setAutoenablesItems(NO);
            let submenu = NSMenu::new(nil).autorelease();
            submenu.setAutoenablesItems(NO);

            let directions = [
                (Direction::Up, language.split_up(), language.move_up()),
                (Direction::Down, language.split_down(), language.move_down()),
                (Direction::Left, language.split_left(), language.move_left()),
                (Direction::Right, language.split_right(), language.move_right()),
            ];
            for (index, (_, split_label, _)) in directions.iter().enumerate() {
                submenu.addItem_(menu_item(split_label, index as isize, true, target));
            }
            submenu.addItem_(NSMenuItem::separatorItem(nil));
            for (index, (_, _, move_label)) in directions.iter().enumerate() {
                submenu.addItem_(menu_item(
                    move_label,
                    (index + 4) as isize,
                    move_enabled[index],
                    target,
                ));
            }

            let parent = NSMenuItem::new(nil).autorelease();
            let title = NSString::alloc(nil).init_str(language.split_and_move());
            let _: () = msg_send![parent, setTitle: title];
            let _: () = msg_send![title, release];
            parent.setSubmenu_(submenu);
            menu.addItem_(parent);

            pop_up_menu(menu, event, view);
            let selected = *(*target).get_ivar::<isize>(SELECTED_COMMAND_IVAR);
            let _: () = msg_send![target, release];
            pool.drain();
            selected
        };

        command_from_tag(selected)
    }

    unsafe fn menu_item(label: &str, tag: isize, enabled: bool, target: id) -> id {
        unsafe {
            let title = NSString::alloc(nil).init_str(label);
            let key = NSString::alloc(nil).init_str("");
            let item = NSMenuItem::alloc(nil)
                .initWithTitle_action_keyEquivalent_(title, sel!(selectEggieTabMenuItem:), key)
                .autorelease();
            let _: () = msg_send![title, release];
            let _: () = msg_send![key, release];
            item.setTarget_(target);
            let _: () = msg_send![item, setTag: tag];
            let _: () = msg_send![item, setEnabled: if enabled { YES } else { NO }];
            item
        }
    }

    /// Attach an SF Symbol (macOS 11+) to a menu item. No-op on failure: an unknown symbol name (or
    /// pre-11 systems) yields a nil image, and `setImage: nil` simply leaves the item icon-less.
    unsafe fn set_menu_item_symbol(item: id, symbol: &str) {
        unsafe {
            let name = NSString::alloc(nil).init_str(symbol);
            let image: id = msg_send![
                class!(NSImage),
                imageWithSystemSymbolName: name
                accessibilityDescription: nil
            ];
            let _: () = msg_send![name, release];
            if image != nil {
                let _: () = msg_send![item, setImage: image];
            }
        }
    }

    fn command_from_tag(tag: isize) -> Option<NativeTabMenuCommand> {
        let direction = match tag.rem_euclid(4) {
            0 => Direction::Up,
            1 => Direction::Down,
            2 => Direction::Left,
            3 => Direction::Right,
            _ => unreachable!(),
        };
        match tag {
            0..=3 => Some(NativeTabMenuCommand::Split(direction)),
            4..=7 => Some(NativeTabMenuCommand::Move(direction)),
            _ => None,
        }
    }

    // --- Project context menu ---------------------------------------------------------------

    pub(crate) struct NativeProjectMenu {
        view: id,
        event: id,
        tab_count: usize,
        language: crate::settings::Language,
    }

    impl NativeProjectMenu {
        pub(super) fn prepare(
            window: &Window,
            tab_count: usize,
            language: crate::settings::Language,
        ) -> Option<Self> {
            let window_handle = HasWindowHandle::window_handle(window).ok()?;
            let RawWindowHandle::AppKit(appkit_handle) = window_handle.as_raw() else {
                return None;
            };
            let view = appkit_handle.ns_view.as_ptr() as id;
            let event: id = unsafe { msg_send![NSApp(), currentEvent] };
            if view == nil || event == nil {
                return None;
            }
            unsafe {
                let _: id = msg_send![view, retain];
                let _: id = msg_send![event, retain];
            }
            Some(Self {
                view,
                event,
                tab_count,
                language,
            })
        }

        pub(crate) fn show(self) -> Option<NativeProjectMenuCommand> {
            show_project_menu(self.view, self.event, self.tab_count, self.language)
        }
    }

    impl Drop for NativeProjectMenu {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.event, release];
                let _: () = msg_send![self.view, release];
            }
        }
    }

    fn show_project_menu(
        view: id,
        event: id,
        tab_count: usize,
        language: crate::settings::Language,
    ) -> Option<NativeProjectMenuCommand> {
        let selected = unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let target: id = msg_send![menu_target_class(), new];
            (*target).set_ivar(SELECTED_COMMAND_IVAR, NO_SELECTION);
            let menu = NSMenu::new(nil).autorelease();
            menu.setAutoenablesItems(NO);

            menu.addItem_(menu_item(language.edit_name(), 0, true, target));
            menu.addItem_(menu_item(language.set_root(), 1, true, target));
            menu.addItem_(NSMenuItem::separatorItem(nil));
            let close_label = language.close_tabs(tab_count);
            menu.addItem_(menu_item(&close_label, 2, tab_count > 0, target));
            menu.addItem_(NSMenuItem::separatorItem(nil));
            menu.addItem_(menu_item(language.delete_project(), 3, true, target));

            pop_up_menu(menu, event, view);
            let selected = *(*target).get_ivar::<isize>(SELECTED_COMMAND_IVAR);
            let _: () = msg_send![target, release];
            pool.drain();
            selected
        };

        project_command_from_tag(selected)
    }

    fn project_command_from_tag(tag: isize) -> Option<NativeProjectMenuCommand> {
        match tag {
            0 => Some(NativeProjectMenuCommand::EditName),
            1 => Some(NativeProjectMenuCommand::SetRoot),
            2 => Some(NativeProjectMenuCommand::CloseTabs),
            3 => Some(NativeProjectMenuCommand::DeleteProject),
            _ => None,
        }
    }

    // --- Process context menu ---------------------------------------------------------------

    pub(crate) struct NativeProcessMenu {
        view: id,
        event: id,
        language: crate::settings::Language,
    }

    impl NativeProcessMenu {
        pub(super) fn prepare(
            window: &Window,
            language: crate::settings::Language,
        ) -> Option<Self> {
            let window_handle = HasWindowHandle::window_handle(window).ok()?;
            let RawWindowHandle::AppKit(appkit_handle) = window_handle.as_raw() else {
                return None;
            };
            let view = appkit_handle.ns_view.as_ptr() as id;
            let event: id = unsafe { msg_send![NSApp(), currentEvent] };
            if view == nil || event == nil {
                return None;
            }
            unsafe {
                let _: id = msg_send![view, retain];
                let _: id = msg_send![event, retain];
            }
            Some(Self { view, event, language })
        }

        pub(crate) fn show(self) -> Option<NativeProcessMenuCommand> {
            show_process_menu(self.view, self.event, self.language)
        }
    }

    impl Drop for NativeProcessMenu {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.event, release];
                let _: () = msg_send![self.view, release];
            }
        }
    }

    fn show_process_menu(
        view: id,
        event: id,
        language: crate::settings::Language,
    ) -> Option<NativeProcessMenuCommand> {
        let selected = unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let target: id = msg_send![menu_target_class(), new];
            (*target).set_ivar(SELECTED_COMMAND_IVAR, NO_SELECTION);
            let menu = NSMenu::new(nil).autorelease();
            menu.setAutoenablesItems(NO);

            menu.addItem_(menu_item(language.terminate(), 0, true, target));
            menu.addItem_(menu_item(language.force_kill(), 1, true, target));
            menu.addItem_(NSMenuItem::separatorItem(nil));
            menu.addItem_(menu_item(language.copy_pid(), 2, true, target));
            menu.addItem_(menu_item(language.copy_executable_path(), 3, true, target));

            pop_up_menu(menu, event, view);
            let selected = *(*target).get_ivar::<isize>(SELECTED_COMMAND_IVAR);
            let _: () = msg_send![target, release];
            pool.drain();
            selected
        };

        process_command_from_tag(selected)
    }

    fn process_command_from_tag(tag: isize) -> Option<NativeProcessMenuCommand> {
        match tag {
            0 => Some(NativeProcessMenuCommand::Terminate),
            1 => Some(NativeProcessMenuCommand::ForceKill),
            2 => Some(NativeProcessMenuCommand::CopyPid),
            3 => Some(NativeProcessMenuCommand::CopyExecutablePath),
            _ => None,
        }
    }


    // --- Snippet context menu ---------------------------------------------------------------

    pub(crate) struct NativeSnippetMenu {
        view: id,
        event: id,
        language: crate::settings::Language,
    }

    impl NativeSnippetMenu {
        pub(super) fn prepare(
            window: &Window,
            language: crate::settings::Language,
        ) -> Option<Self> {
            let window_handle = HasWindowHandle::window_handle(window).ok()?;
            let RawWindowHandle::AppKit(appkit_handle) = window_handle.as_raw() else {
                return None;
            };
            let view = appkit_handle.ns_view.as_ptr() as id;
            let event: id = unsafe { msg_send![NSApp(), currentEvent] };
            if view == nil || event == nil {
                return None;
            }
            unsafe {
                let _: id = msg_send![view, retain];
                let _: id = msg_send![event, retain];
            }
            Some(Self { view, event, language })
        }

        pub(crate) fn show(self) -> Option<NativeSnippetMenuCommand> {
            show_snippet_menu(self.view, self.event, self.language)
        }
    }

    impl Drop for NativeSnippetMenu {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.event, release];
                let _: () = msg_send![self.view, release];
            }
        }
    }

    fn show_snippet_menu(
        view: id,
        event: id,
        language: crate::settings::Language,
    ) -> Option<NativeSnippetMenuCommand> {
        let selected = unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let target: id = msg_send![menu_target_class(), new];
            (*target).set_ivar(SELECTED_COMMAND_IVAR, NO_SELECTION);
            let menu = NSMenu::new(nil).autorelease();
            menu.setAutoenablesItems(NO);

            menu.addItem_(menu_item(language.use_snippet(), 0, true, target));
            menu.addItem_(menu_item(language.edit_snippet(), 1, true, target));
            menu.addItem_(NSMenuItem::separatorItem(nil));
            menu.addItem_(menu_item(language.delete_snippet(), 2, true, target));

            pop_up_menu(menu, event, view);
            let selected = *(*target).get_ivar::<isize>(SELECTED_COMMAND_IVAR);
            let _: () = msg_send![target, release];
            pool.drain();
            selected
        };

        snippet_command_from_tag(selected)
    }

    fn snippet_command_from_tag(tag: isize) -> Option<NativeSnippetMenuCommand> {
        match tag {
            0 => Some(NativeSnippetMenuCommand::Use),
            1 => Some(NativeSnippetMenuCommand::Edit),
            2 => Some(NativeSnippetMenuCommand::Delete),
            _ => None,
        }
    }
    // --- Edit context menu ------------------------------------------------------------------

    pub(crate) struct NativeEditMenu {
        view: id,
        event: id,
        /// Per-item enabled flags in tag order: [cut, copy, paste, select_all].
        enabled: [bool; 4],
        language: crate::settings::Language,
    }

    impl NativeEditMenu {
        pub(super) fn prepare(
            window: &Window,
            enabled: [bool; 4],
            language: crate::settings::Language,
        ) -> Option<Self> {
            let window_handle = HasWindowHandle::window_handle(window).ok()?;
            let RawWindowHandle::AppKit(appkit_handle) = window_handle.as_raw() else {
                return None;
            };
            let view = appkit_handle.ns_view.as_ptr() as id;
            let event: id = unsafe { msg_send![NSApp(), currentEvent] };
            if view == nil || event == nil {
                return None;
            }
            unsafe {
                let _: id = msg_send![view, retain];
                let _: id = msg_send![event, retain];
            }
            Some(Self {
                view,
                event,
                enabled,
                language,
            })
        }

        pub(crate) fn show(self) -> Option<NativeEditMenuCommand> {
            show_edit_menu(self.view, self.event, self.enabled, self.language)
        }
    }

    impl Drop for NativeEditMenu {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.event, release];
                let _: () = msg_send![self.view, release];
            }
        }
    }

    fn show_edit_menu(
        view: id,
        event: id,
        enabled: [bool; 4],
        language: crate::settings::Language,
    ) -> Option<NativeEditMenuCommand> {
        let selected = unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let target: id = msg_send![menu_target_class(), new];
            (*target).set_ivar(SELECTED_COMMAND_IVAR, NO_SELECTION);
            let menu = NSMenu::new(nil).autorelease();
            menu.setAutoenablesItems(NO);

            // SF Symbols matching each action, for a more native, modern look.
            let cut = menu_item(language.cut(), 0, enabled[0], target);
            set_menu_item_symbol(cut, "scissors");
            menu.addItem_(cut);
            let copy = menu_item(language.copy(), 1, enabled[1], target);
            set_menu_item_symbol(copy, "doc.on.doc");
            menu.addItem_(copy);
            let paste = menu_item(language.paste(), 2, enabled[2], target);
            set_menu_item_symbol(paste, "doc.on.clipboard");
            menu.addItem_(paste);
            menu.addItem_(NSMenuItem::separatorItem(nil));
            let select_all = menu_item(language.select_all(), 3, enabled[3], target);
            set_menu_item_symbol(select_all, "text.justify");
            menu.addItem_(select_all);

            pop_up_menu(menu, event, view);
            let selected = *(*target).get_ivar::<isize>(SELECTED_COMMAND_IVAR);
            let _: () = msg_send![target, release];
            pool.drain();
            selected
        };

        edit_command_from_tag(selected)
    }

    fn edit_command_from_tag(tag: isize) -> Option<NativeEditMenuCommand> {
        match tag {
            0 => Some(NativeEditMenuCommand::Cut),
            1 => Some(NativeEditMenuCommand::Copy),
            2 => Some(NativeEditMenuCommand::Paste),
            3 => Some(NativeEditMenuCommand::SelectAll),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{edit_command_from_tag, NativeEditMenuCommand};

        #[test]
        fn edit_command_from_tag_decodes_each_item() {
            assert_eq!(edit_command_from_tag(0), Some(NativeEditMenuCommand::Cut));
            assert_eq!(edit_command_from_tag(1), Some(NativeEditMenuCommand::Copy));
            assert_eq!(edit_command_from_tag(2), Some(NativeEditMenuCommand::Paste));
            assert_eq!(
                edit_command_from_tag(3),
                Some(NativeEditMenuCommand::SelectAll)
            );
            // NO_SELECTION(-1)与越界 tag 解码为 None(用户点空白/关闭菜单)。
            assert_eq!(edit_command_from_tag(-1), None);
            assert_eq!(edit_command_from_tag(4), None);
        }
    }
}
