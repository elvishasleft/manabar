use quotabar_core::model::{ProviderKind, ProviderView};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;

pub fn tooltip_string(views: &[ProviderView]) -> String {
    let name = |k: ProviderKind| match k {
        ProviderKind::Claude => "Claude",
        ProviderKind::Codex => "Codex",
        ProviderKind::Grok => "Grok",
    };
    views
        .iter()
        .map(|v| match v.remaining_percent {
            Some(p) => format!("{} {}%", name(v.kind), p.round() as i64),
            None => format!("{} —", name(v.kind)),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn remaining_array(views: &[ProviderView]) -> [Option<f64>; 3] {
    let get =
        |k: ProviderKind| views.iter().find(|v| v.kind == k).and_then(|v| v.remaining_percent);
    [get(ProviderKind::Claude), get(ProviderKind::Codex), get(ProviderKind::Grok)]
}

pub fn update_tray(app: &tauri::AppHandle, views: &[ProviderView]) {
    if let Some(tray) = app.tray_by_id("main") {
        let rgba = crate::icon::render_tray_icon(remaining_array(views));
        let _ =
            tray.set_icon(Some(Image::new_owned(rgba, crate::icon::ICON_SIZE, crate::icon::ICON_SIZE)));
        let _ = tray.set_tooltip(Some(tooltip_string(views)));
    }
}

pub fn toggle_panel(app: &tauri::AppHandle) {
    use tauri_plugin_positioner::{Position, WindowExt};
    if let Some(w) = app.get_webview_window("panel") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            let _ = w.move_window(Position::TrayBottomCenter);
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

pub fn create_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let refresh = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let auto_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart =
        CheckMenuItem::with_id(app, "autostart", "Start with Windows", true, auto_on, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = MenuBuilder::new(app).items(&[&refresh, &autostart, &quit]).build()?;
    let rgba = crate::icon::render_tray_icon([None, None, None]);
    TrayIconBuilder::with_id("main")
        .icon(Image::new_owned(rgba, crate::icon::ICON_SIZE, crate::icon::ICON_SIZE))
        .tooltip("QuotaBar — starting…")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "quit" => app.exit(0),
            "refresh" => {
                let shared = app.state::<crate::AppShared>();
                shared.request_refresh();
            }
            "autostart" => {
                let al = app.autolaunch();
                if al.is_enabled().unwrap_or(false) {
                    let _ = al.disable();
                } else {
                    let _ = al.enable();
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            tauri_plugin_positioner::on_tray_event(tray.app_handle(), &event);
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_panel(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use quotabar_core::model::{Health, ProviderKind};
    use quotabar_core::providers::initial_view;

    #[test]
    fn tooltip_shows_percent_or_dash() {
        let mut v1 = initial_view(ProviderKind::Claude);
        v1.remaining_percent = Some(82.4);
        v1.health = Health::Green;
        let v2 = initial_view(ProviderKind::Codex);
        let mut v3 = initial_view(ProviderKind::Grok);
        v3.remaining_percent = Some(95.0);
        v3.health = Health::Green;
        assert_eq!(tooltip_string(&[v1, v2, v3]), "Claude 82% · Codex — · Grok 95%");
    }
}
