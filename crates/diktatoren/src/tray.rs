//! The tray icon and its menu.

use crate::bus::{self, Action, BusEvent};
use crate::i18n::tr;
use crate::icon::{self, State};

const TOOLTIP: &str = "Diktatoren";

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use ksni::blocking::{Handle, TrayMethods};

    struct Item {
        state: State,
    }

    impl ksni::Tray for Item {
        fn id(&self) -> String {
            "diktatoren".into()
        }
        fn title(&self) -> String {
            TOOLTIP.into()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            [22, 32, 48, 64]
                .into_iter()
                .map(|size| ksni::Icon {
                    width: size as i32,
                    height: size as i32,
                    data: icon::argb(size, self.state),
                })
                .collect()
        }
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip { title: TOOLTIP.into(), ..Default::default() }
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            bus::send(BusEvent::Action(Action::Toggle));
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            let item = |label: &str, action: Action| {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |_: &mut Self| bus::send(BusEvent::Action(action))),
                    ..Default::default()
                }
                .into()
            };
            vec![
                item(tr("Dictate"), Action::Toggle),
                item(tr("Settings…"), Action::Settings),
                ksni::MenuItem::Separator,
                item(tr("Quit"), Action::Quit),
            ]
        }
    }

    pub struct Tray(Option<Handle<Item>>);

    impl Tray {
        pub fn new() -> Self {
            match (Item { state: State::Idle }).spawn() {
                Ok(handle) => Tray(Some(handle)),
                Err(e) => {
                    log::warn!("no system tray: {e}");
                    Tray(None)
                }
            }
        }

        pub fn set_state(&self, state: State) {
            if let Some(h) = &self.0 {
                h.update(|item| item.state = state);
            }
        }

        /// Rebuilds the menu in the current interface language.
        pub fn relabel(&self) {
            if let Some(h) = &self.0 {
                h.update(|_| {});
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Tray(Option<TrayIcon>, [MenuItem; 3]);

    fn tray_icon(state: State) -> Option<tray_icon::Icon> {
        tray_icon::Icon::from_rgba(icon::rgba(32, state), 32, 32).ok()
    }

    impl Tray {
        /// Must run on the main thread with the event loop running.
        pub fn new() -> Self {
            let dictate = MenuItem::new(tr("Dictate"), true, None);
            let settings = MenuItem::new(tr("Settings…"), true, None);
            let quit = MenuItem::new(tr("Quit"), true, None);
            let menu = Menu::new();
            let _ = menu.append_items(&[&dictate, &settings, &PredefinedMenuItem::separator(), &quit]);
            let ids = [
                (dictate.id().clone(), Action::Toggle),
                (settings.id().clone(), Action::Settings),
                (quit.id().clone(), Action::Quit),
            ];
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                if let Some((_, action)) = ids.iter().find(|(id, _)| *id == event.id) {
                    bus::send(BusEvent::Action(*action));
                }
            }));
            TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    button: tray_icon::MouseButton::Left,
                    button_state: tray_icon::MouseButtonState::Up,
                    ..
                } = event
                {
                    bus::send(BusEvent::Action(Action::Toggle));
                }
            }));
            let mut builder = TrayIconBuilder::new()
                .with_tooltip(TOOLTIP)
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false);
            if let Some(icon) = tray_icon(State::Idle) {
                builder = builder.with_icon(icon);
            }
            let items = [dictate, settings, quit];
            match builder.build() {
                Ok(t) => Tray(Some(t), items),
                Err(e) => {
                    log::warn!("no system tray: {e}");
                    Tray(None, items)
                }
            }
        }

        pub fn set_state(&self, state: State) {
            if let Some(t) = &self.0 {
                let _ = t.set_icon(tray_icon(state));
            }
        }

        /// Relabels the menu in the current interface language.
        pub fn relabel(&self) {
            for (item, label) in self.1.iter().zip(["Dictate", "Settings…", "Quit"]) {
                item.set_text(tr(label));
            }
        }
    }
}

pub use imp::Tray;
