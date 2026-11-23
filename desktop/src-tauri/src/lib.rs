mod control;
mod daemon;
mod status;

use std::sync::Arc;

use tauri::{Manager, State};
use tokio::sync::watch;

use crate::control::ControlBridge;
use crate::daemon::{AppRuntime, spawn_status_emitter};
use crate::status::{SharedStatusHub, StatusHub};

#[tauri::command]
async fn desktop_status(runtime: State<'_, Arc<AppRuntime>>) -> Result<status::AppStatus, String> {
    Ok(runtime.status.snapshot())
}

#[tauri::command]
async fn restart_daemon(runtime: State<'_, Arc<AppRuntime>>) -> Result<(), String> {
    runtime.restart().await
}

fn setup(app: &mut tauri::App) -> Result<(), String> {
    let control_port = daemon::select_control_port()?;
    let base_url = format!("http://127.0.0.1:{control_port}");
    let status_hub: SharedStatusHub = Arc::new(StatusHub::new());
    let (restart_tx, restart_rx) = watch::channel(0u64);

    let runtime = Arc::new(AppRuntime::new(
        app.handle().clone(),
        Arc::clone(&status_hub),
        control_port,
        restart_tx,
    ));
    app.manage(Arc::clone(&runtime));

    spawn_status_emitter(app.handle().clone(), Arc::clone(&status_hub));
    tauri::async_runtime::block_on(runtime.start())?;

    let bridge = ControlBridge::new(base_url, Arc::clone(&status_hub), restart_rx);
    tauri::async_runtime::spawn(bridge.run());

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![desktop_status, restart_daemon])
        .setup(|app| {
            setup(app).map_err(|error| anyhow::anyhow!(error))?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Seneschal desktop app")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(runtime) = app_handle.try_state::<Arc<AppRuntime>>()
            {
                tauri::async_runtime::block_on(runtime.shutdown());
            }
        });
}
