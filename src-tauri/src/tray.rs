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

/// Single-letter provider abbreviation used in the macOS menu bar title —
/// deliberately distinct from `provider_name`'s full names since the title
/// has to fit in a narrow menu bar slot. Kept as a plain, cross-platform
/// pure function (exercised by unit tests on every CI target) even though
/// its only call site outside tests is macOS-gated; `allow(dead_code)` on
/// non-macOS targets acknowledges that intentional asymmetry rather than
/// hiding a real one.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn provider_letter(kind: ProviderKind) -> char {
    match kind {
        ProviderKind::Claude => 'C',
        ProviderKind::Codex => 'X',
        ProviderKind::Grok => 'G',
        ProviderKind::DeepSeek => 'D',
    }
}

/// Builds the macOS menu bar title text, e.g. `"C 82%"`, from the *binding*
/// provider among enabled views — the one with the lowest remaining percent.
/// An unavailable provider (`remaining_percent == None`) is treated as -1 so
/// it always outranks (wins binding status over) any numeric reading; that
/// means an expired/unavailable provider becomes the binding one and the
/// title reads `"{L} —"`. If *every* enabled view is unavailable the title
/// is a plain `"—"` (no single letter is more meaningful than another).
/// With no enabled providers at all the title is `""`, mirroring
/// `render_tray_icon`'s all-disabled case of drawing no bars. Pure and
/// platform-agnostic (unit-tested on every CI target) even though its only
/// non-test call site — inside `update_tray` — is macOS-gated; see
/// `provider_letter` for why `allow(dead_code)` is scoped to non-macOS.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn menubar_title(views: &[ProviderView]) -> String {
    let enabled: Vec<&ProviderView> = views.iter().filter(|v| v.enabled).collect();
    let Some(binding) = enabled.iter().min_by(|a, b| {
        let ra = a.remaining_percent.unwrap_or(-1.0);
        let rb = b.remaining_percent.unwrap_or(-1.0);
        ra.total_cmp(&rb)
    }) else {
        return String::new();
    };
    match binding.remaining_percent {
        Some(pct) => format!("{} {}%", provider_letter(binding.kind), pct.round() as i64),
        None if enabled.iter().all(|v| v.remaining_percent.is_none()) => "—".to_string(),
        None => format!("{} —", provider_letter(binding.kind)),
    }
}

pub fn update_tray(app: &tauri::AppHandle, views: &[ProviderView], _menubar_text: bool) {
    if let Some(tray) = app.tray_by_id("main") {
        let rgba = crate::icon::render_tray_icon(&enabled_bars(views));
        let _ = tray.set_icon(Some(Image::new_owned(
            rgba,
            crate::icon::ICON_SIZE,
            crate::icon::ICON_SIZE,
        )));
        let _ = tray.set_tooltip(Some(tooltip_string(views)));
        // Menu bar text is a macOS-only concept (Tauri's `set_title` is a
        // no-op/unsupported on Windows) and updates every time the icon
        // does, per the same poll-driven cadence.
        #[cfg(target_os = "macos")]
        {
            if _menubar_text {
                let _ = tray.set_title(Some(menubar_title(views)));
            } else {
                let _ = tray.set_title(None::<String>);
            }
        }
    }
}

/// Where the panel anchors relative to the tray icon: `BottomRight` for a
/// Windows taskbar tray (icon at the bottom of the screen), `TopRight` for
/// a macOS menu bar (icon at the top). Chosen per-platform by the
/// `PANEL_ANCHOR` const below, and taken as an explicit parameter here so
/// `panel_position` itself stays a pure, platform-agnostic function
/// (unit-tested for both variants on every CI target, even though only one
/// variant is ever constructed by non-test code on a given platform).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // each platform constructs only its own variant; both are unit-tested
pub(crate) enum Anchor {
    BottomRight,
    TopRight,
}

#[cfg(target_os = "macos")]
pub(crate) const PANEL_ANCHOR: Anchor = Anchor::TopRight;
#[cfg(not(target_os = "macos"))]
pub(crate) const PANEL_ANCHOR: Anchor = Anchor::BottomRight;

/// Computes the top-left corner (in physical pixels) for placing the panel
/// window relative to a monitor's work area, leaving a fixed margin so the
/// window doesn't touch the taskbar/menu bar or screen edge.
///
/// `work_pos` / `work_size` describe the monitor's work area (the region
/// excluding the taskbar or menu bar); `win_size` is the panel window's own
/// size. `x` is always right-aligned (tray icons live in the top-right menu
/// bar on macOS and typically the bottom-right taskbar corner on Windows);
/// `y` depends on `anchor`: `BottomRight` sits above the taskbar,
/// `TopRight` sits just below the menu bar. The result is clamped so the
/// window never starts above/left of the work area's origin, which matters
/// when the window is taller or wider than the work area itself.
pub(crate) fn panel_position(
    work_pos: (i32, i32),
    work_size: (u32, u32),
    win_size: (u32, u32),
    anchor: Anchor,
) -> (i32, i32) {
    const MARGIN: i32 = 12;
    let x = work_pos.0 + work_size.0 as i32 - win_size.0 as i32 - MARGIN;
    let y = match anchor {
        Anchor::BottomRight => work_pos.1 + work_size.1 as i32 - win_size.1 as i32 - MARGIN,
        Anchor::TopRight => work_pos.1 + MARGIN,
    };
    (x.max(work_pos.0), y.max(work_pos.1))
}

/// Shows, positions, and focuses the panel window — the "make it visible
/// and give it focus" half of `toggle_panel`, factored out so the
/// single-instance handler (a second launch should focus the existing
/// panel, not toggle it shut if it happened to already be open) and
/// `toggle_panel` itself can share the exact same show behavior.
pub fn show_panel(app: &tauri::AppHandle) {
    use tauri::Emitter;
    if let Some(w) = app.get_webview_window("panel") {
        if let (Ok(Some(monitor)), Ok(win_size)) = (w.current_monitor(), w.outer_size()) {
            let work_area = monitor.work_area();
            let (x, y) = panel_position(
                (work_area.position.x, work_area.position.y),
                (work_area.size.width, work_area.size.height),
                (win_size.width, win_size.height),
                PANEL_ANCHOR,
            );
            let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
        }
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit("panel-shown", ());
    }
}

pub fn toggle_panel(app: &tauri::AppHandle) {
    let is_visible = app
        .get_webview_window("panel")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if is_visible {
        if let Some(w) = app.get_webview_window("panel") {
            let _ = w.hide();
        }
    } else {
        show_panel(app);
    }
}

pub fn create_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let refresh = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let auto_on = app.autolaunch().is_enabled().unwrap_or(false);
    #[cfg(target_os = "macos")]
    let autostart_label = "Start at Login";
    #[cfg(not(target_os = "macos"))]
    let autostart_label = "Start with Windows";
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        autostart_label,
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
        let pos = panel_position((0, 0), (1920, 1032), (380, 560), Anchor::BottomRight);
        assert_eq!(pos, (1528, 460));
    }

    #[test]
    fn panel_position_respects_non_zero_work_area_origin() {
        // Secondary monitor to the right of the primary, work area origin
        // is offset accordingly.
        let pos = panel_position((1920, 0), (1920, 1032), (380, 560), Anchor::BottomRight);
        assert_eq!(pos, (3448, 460));
    }

    #[test]
    fn panel_position_clamps_when_window_taller_than_work_area() {
        // Window taller than the work area must clamp to the work area's
        // origin rather than producing a negative/off-screen y.
        let pos = panel_position((0, 0), (1920, 500), (380, 560), Anchor::BottomRight);
        assert_eq!(pos, (1528, 0));
    }

    #[test]
    fn panel_position_clamps_when_window_wider_than_work_area() {
        let pos = panel_position((0, 0), (300, 1032), (380, 560), Anchor::BottomRight);
        assert_eq!(pos, (0, 460));
    }

    #[test]
    fn panel_position_top_right_sits_below_menu_bar() {
        // macOS anchor: x uses the same right-alignment math as
        // BottomRight; y is simply work-area top + the 12px margin.
        let pos = panel_position((0, 0), (1920, 1032), (380, 560), Anchor::TopRight);
        assert_eq!(pos, (1528, 12));
    }

    #[test]
    fn panel_position_top_right_respects_non_zero_work_area_origin() {
        let pos = panel_position((1920, 0), (1920, 1032), (380, 560), Anchor::TopRight);
        assert_eq!(pos, (3448, 12));
    }

    #[test]
    fn panel_position_top_right_clamps_to_work_area_origin() {
        // A non-zero work-area top (e.g. a secondary monitor stacked below
        // the primary) still clamps y to at least the work area's own top.
        let pos = panel_position((0, 100), (1920, 1032), (380, 560), Anchor::TopRight);
        assert_eq!(pos, (1528, 112));
    }

    #[test]
    fn menubar_title_shows_binding_provider_percent() {
        let mut v1 = initial_view(ProviderKind::Claude);
        v1.remaining_percent = Some(82.4);
        let mut v2 = initial_view(ProviderKind::Codex);
        v2.remaining_percent = Some(35.0);
        let mut v3 = initial_view(ProviderKind::Grok);
        v3.remaining_percent = Some(95.0);
        assert_eq!(
            menubar_title(&[v1, v2, v3]),
            "X 35%",
            "Codex has the lowest remaining percent, so it's the binding provider"
        );
    }

    #[test]
    fn menubar_title_shows_dash_when_binding_provider_unavailable() {
        let mut v1 = initial_view(ProviderKind::Claude);
        v1.remaining_percent = Some(82.4);
        let v2 = initial_view(ProviderKind::Codex); // remaining_percent stays None
        assert_eq!(
            menubar_title(&[v1, v2]),
            "X —",
            "an unavailable enabled provider outranks any numeric reading and becomes binding"
        );
    }

    #[test]
    fn menubar_title_all_unavailable_shows_plain_dash() {
        let v1 = initial_view(ProviderKind::Claude);
        let v2 = initial_view(ProviderKind::Codex);
        assert_eq!(menubar_title(&[v1, v2]), "—");
    }

    #[test]
    fn menubar_title_empty_when_no_enabled_providers() {
        let mut v1 = initial_view(ProviderKind::Claude);
        v1.enabled = false;
        assert_eq!(menubar_title(&[v1]), "");
    }
}
