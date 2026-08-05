use eggie_domain::Direction;
use gpui::Window;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeTabMenuCommand {
    Split(Direction),
    Move(Direction),
}

#[cfg(target_os = "macos")]
pub(crate) use macos::NativeTabMenu;

#[cfg(target_os = "macos")]
pub(crate) fn prepare_tab_menu(window: &Window, move_enabled: [bool; 4]) -> Option<NativeTabMenu> {
    macos::NativeTabMenu::prepare(window, move_enabled)
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
pub(crate) fn prepare_tab_menu(_: &Window, _: [bool; 4]) -> Option<NativeTabMenu> {
    None
}

#[cfg(target_os = "macos")]
mod macos {
    use super::NativeTabMenuCommand;
    use cocoa::{
        appkit::{NSApp, NSMenu, NSMenuItem},
        base::{NO, YES, id, nil},
        foundation::{NSAutoreleasePool, NSString},
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
    }

    impl NativeTabMenu {
        pub(super) fn prepare(window: &Window, move_enabled: [bool; 4]) -> Option<Self> {
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
            })
        }

        /// Show the modal AppKit menu without holding a GPUI `App`/entity borrow.
        ///
        /// `NSMenu` runs a nested event loop. Calling this from a GPUI listener lets unrelated
        /// foreground tasks re-enter GPUI while its `RefCell` is already mutably borrowed.
        pub(crate) fn show(self) -> Option<NativeTabMenuCommand> {
            show_tab_menu(self.view, self.event, self.move_enabled)
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

    fn show_tab_menu(view: id, event: id, move_enabled: [bool; 4]) -> Option<NativeTabMenuCommand> {
        let selected = unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let target: id = msg_send![menu_target_class(), new];
            (*target).set_ivar(SELECTED_COMMAND_IVAR, NO_SELECTION);
            let menu = NSMenu::new(nil).autorelease();
            menu.setAutoenablesItems(NO);
            let submenu = NSMenu::new(nil).autorelease();
            submenu.setAutoenablesItems(NO);

            let directions = [
                (Direction::Up, "向上拆分", "上移"),
                (Direction::Down, "向下拆分", "下移"),
                (Direction::Left, "向左拆分", "左移"),
                (Direction::Right, "向右拆分", "右移"),
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
            let title = NSString::alloc(nil).init_str("拆分和移动");
            let _: () = msg_send![parent, setTitle: title];
            let _: () = msg_send![title, release];
            parent.setSubmenu_(submenu);
            menu.addItem_(parent);

            let _: () =
                msg_send![class!(NSMenu), popUpContextMenu: menu withEvent: event forView: view];
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
}
