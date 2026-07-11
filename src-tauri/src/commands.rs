use quotabar_core::model::ProviderView;

#[tauri::command]
pub async fn get_state(
    shared: tauri::State<'_, crate::AppShared>,
) -> Result<Vec<ProviderView>, String> {
    Ok(shared.views().await)
}

#[tauri::command]
pub async fn refresh_now(shared: tauri::State<'_, crate::AppShared>) -> Result<(), String> {
    shared.request_refresh();
    Ok(())
}

#[tauri::command]
pub async fn panel_opened(
    shared: tauri::State<'_, crate::AppShared>,
) -> Result<Vec<ProviderView>, String> {
    shared.refresh_usage().await;
    shared.request_refresh();
    Ok(shared.views().await)
}
