use quotabar_core::model::{Health, ProviderKind, ProviderView};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, MenuBuilder, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;

/// Display name for a provider, shared by the tray tooltip and the
/// low-quota notification body so both read the same names.
pub fn provider_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Claude => "Claude",
        ProviderKind::Codex => "Codex",
        ProviderKind::Grok => "Grok",
        ProviderKind::DeepSeek => "DeepSeek",
    }
}

/// Disabled providers are omitted entirely, not shown as a dash — they're
/// meant to disappear from the tray, not just read as unavailable.
pub fn tooltip_string(views: &[ProviderView]) -> String {
    views
        .iter()
        .filter(|v| v.enabled)
        .map(|v| match v.remaining_percent {
            Some(p) => format!("{} {}%", provider_name(v.kind), p.round() as i64),
            None => format!("{} —", provider_name(v.kind)),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// One `(remaining_percent, health)` pair per *enabled* provider, in the
/// same order they appear in `views` (which is `default_providers`' fixed
/// Claude/Codex/Grok/DeepSeek order) — disabled providers contribute no
/// entry at all, so `icon::render_tray_icon` draws no bar for them.
fn enabled_bars(views: &[ProviderView]) -> Vec<(Option<f64>, Health)> {
    views
        .iter()
        .filter(|v| v.enabled)
        .map(|v| (v.remaining_percent, v.health))
        .collect()
}

pub fn update_tray(app: &tauri::AppHandle, views: &[ProviderView]) {
    if let Some(tray) = app.tray_by_id("main") {
        let rgba = crate::icon::render_tray_icon(&enabled_bars(views));
        let _ = tray.set_icon(Some(Image::new_owned(
            rgba,
            crate::icon::ICON_SIZE,
            crate::icon::ICON_SIZE,
        )));
        let _ = tray.set_tooltip(Some(tooltip_string(views)));
    }
}

/// Computes the top-left corner (in physical pixels) for placing the panel
/// window at the bottom-right of a monitor's work area, leaving a fixed
/// margin so the window doesn't touch the taskbar or screen edge.
///
/// `work_pos` / `work_size` describe the monitor's work area (the region
/// excluding the taskbar); `win_size` is the panel window's own size. The
/// result is clamped so the window never starts above/left of the work
/// area's origin, which matters when the window is taller or wider than
/// the work area itself.
pub(crate) fn panel_position(
    work_pos: (i32, i32),
    work_size: (u32, u32),
    win_size: (u32, u32),
) -> (i32, i32) {
    const MARGIN: i32 = 12;
    let x = work_pos.0 + work_size.0 as i32 - win_size.0 as i32 - MARGIN;
    let y = work_pos.1 + work_size.1 as i32 - win_size.1 as i32 - MARGIN;
    (x.max(work_pos.0), y.max(work_pos.1))
}

pub fn toggle_panel(app: &tauri::AppHandle) {
    use tauri::Emitter;
    if let Some(w) = app.get_webview_window("panel") {
        if w.is_visible().unwrap_or(false) {
            let _ = w.hide();
        } else {
            if let (Ok(Some(monitor)), Ok(win_size)) = (w.current_monitor(), w.outer_size()) {
                let work_area = monitor.work_area();
                let (x, y) = panel_position(
                    (work_area.position.x, work_area.position.y),
                    (work_area.size.width, work_area.size.height),
                    (win_size.width, win_size.height),
                );
                let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
            }
            let _ = w.show();
            let _ = w.set_focus();
            let _ = app.emit("panel-shown", ());
        }
    }
}

pub fn create_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let refresh = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let auto_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start with Windows",
        true,
        auto_on,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = MenuBuilder::new(app)
        .items(&[&refresh, &autostart, &quit])
        .build()?;
    // Placeholder icon shown before the first poll completes. Sized to the
    // number of currently-enabled providers (AppShared is already managed
    // by the time create_tray runs) so a fully-disabled config doesn't
    // briefly flash 4 gray bars before poll_and_publish redraws it.
    let shared = app.state::<crate::AppShared>();
    let placeholder = vec![(None, Health::Unavailable); shared.enabled_count()];
    let rgba = crate::icon::render_tray_icon(&placeholder);
    TrayIconBuilder::with_id("main")
        .icon(Image::new_owned(
            rgba,
            crate::icon::ICON_SIZE,
            crate::icon::ICON_SIZE,
        ))
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
        let mut v4 = initial_view(ProviderKind::DeepSeek);
        v4.remaining_percent = Some(60.0);
        v4.health = Health::Green;
        assert_eq!(
            tooltip_string(&[v1, v2, v3, v4]),
            "Claude 82% · Codex — · Grok 95% · DeepSeek 60%"
        );
    }

    #[test]
    fn tooltip_omits_disabled_providers_entirely() {
        let mut v1 = initial_view(ProviderKind::Claude);
        v1.remaining_percent = Some(82.4);
        v1.health = Health::Green;
        let mut v2 = initial_view(ProviderKind::Codex);
        v2.enabled = false; // disabled: must be absent, not "Codex —"
        let mut v3 = initial_view(ProviderKind::Grok);
        v3.remaining_percent = Some(95.0);
        v3.health = Health::Green;
        assert_eq!(
            tooltip_string(&[v1, v2, v3]),
            "Claude 82% · Grok 95%",
            "disabled provider must not appear at all"
        );
    }

    #[test]
    fn enabled_bars_filters_out_disabled_providers() {
        let mut v1 = initial_view(ProviderKind::Claude);
        v1.remaining_percent = Some(50.0);
        v1.health = Health::Green;
        let mut v2 = initial_view(ProviderKind::Codex);
        v2.enabled = false;
        let bars = enabled_bars(&[v1, v2]);
        assert_eq!(bars, vec![(Some(50.0), Health::Green)]);
    }

    #[test]
    fn panel_position_sits_bottom_right_above_taskbar() {
        // Typical 1920x1032 work area (1920x1080 monitor, 48px taskbar) with
        // a 380x560 panel: bottom-right corner minus the 12px margin.
        let pos = panel_position((0, 0), (1920, 1032), (380, 560));
        assert_eq!(pos, (1528, 460));
    }

    #[test]
    fn panel_position_respects_non_zero_work_area_origin() {
        // Secondary monitor to the right of the primary, work area origin
        // is offset accordingly.
        let pos = panel_position((1920, 0), (1920, 1032), (380, 560));
        assert_eq!(pos, (3448, 460));
    }

    #[test]
    fn panel_position_clamps_when_window_taller_than_work_area() {
        // Window taller than the work area must clamp to the work area's
        // origin rather than producing a negative/off-screen y.
        let pos = panel_position((0, 0), (1920, 500), (380, 560));
        assert_eq!(pos, (1528, 0));
    }

    #[test]
    fn panel_position_clamps_when_window_wider_than_work_area() {
        let pos = panel_position((0, 0), (300, 1032), (380, 560));
        assert_eq!(pos, (0, 460));
    }
}
