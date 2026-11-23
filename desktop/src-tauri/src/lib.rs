mod audio;
mod control;
mod daemon;
mod models;
mod settings;
mod status;

use std::sync::Arc;
use std::time::Duration;

use tauri::{Manager, State};
use tokio::sync::watch;

use crate::audio::SharedAudioHub;
use crate::control::ControlBridge;
use crate::daemon::{AppRuntime, spawn_status_emitter};
use crate::models::SharedModelHub;
use crate::status::{SharedStatusHub, StatusHub};

#[tauri::command]
async fn desktop_status(runtime: State<'_, Arc<AppRuntime>>) -> Result<status::AppStatus, String> {
    Ok(runtime.status.snapshot())
}

#[tauri::command]
async fn restart_daemon(runtime: State<'_, Arc<AppRuntime>>) -> Result<(), String> {
    runtime.restart().await
}

#[tauri::command]
async fn get_settings(runtime: State<'_, Arc<AppRuntime>>) -> Result<serde_json::Value, String> {
    settings::get_config(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn validate_settings(
    runtime: State<'_, Arc<AppRuntime>>,
    patch: serde_json::Value,
) -> Result<serde_json::Value, String> {
    settings::validate_config(&runtime.base_url(), &patch)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn apply_settings(
    runtime: State<'_, Arc<AppRuntime>>,
    patch: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let result = settings::apply_config(&runtime.base_url(), &patch)
        .await
        .map_err(|error| error.to_string())?;
    // The wire token is `restart` (issue #33 ADR originally said
    // `restart_required`; the implementation and TS types use `restart`).
    if result.get("tier").and_then(|tier| tier.as_str()) == Some("restart") {
        runtime.restart().await?;
        runtime
            .status
            .wait_until_connected(Duration::from_secs(30))
            .await?;
    }
    Ok(result)
}

#[tauri::command]
async fn get_audio_devices(
    runtime: State<'_, Arc<AppRuntime>>,
) -> Result<serde_json::Value, String> {
    audio::devices(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_audio_status(
    runtime: State<'_, Arc<AppRuntime>>,
) -> Result<serde_json::Value, String> {
    audio::status(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn request_audio_permission(
    runtime: State<'_, Arc<AppRuntime>>,
) -> Result<serde_json::Value, String> {
    audio::request_permission(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn tts_preview(runtime: State<'_, Arc<AppRuntime>>) -> Result<serde_json::Value, String> {
    audio::tts_preview(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn speaker_test(runtime: State<'_, Arc<AppRuntime>>) -> Result<serde_json::Value, String> {
    audio::speaker_test(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn mic_test(
    runtime: State<'_, Arc<AppRuntime>>,
    duration_secs: Option<u64>,
) -> Result<serde_json::Value, String> {
    audio::mic_test(&runtime.base_url(), duration_secs)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn loopback_test(
    runtime: State<'_, Arc<AppRuntime>>,
    duration_secs: Option<u64>,
) -> Result<serde_json::Value, String> {
    audio::loopback_test(&runtime.base_url(), duration_secs)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_models(runtime: State<'_, Arc<AppRuntime>>) -> Result<serde_json::Value, String> {
    models::catalog(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn download_model(
    runtime: State<'_, Arc<AppRuntime>>,
    id: String,
) -> Result<serde_json::Value, String> {
    models::download_model(&runtime.base_url(), &id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn cancel_model_download(
    runtime: State<'_, Arc<AppRuntime>>,
    id: String,
) -> Result<serde_json::Value, String> {
    models::cancel_model_download(&runtime.base_url(), &id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn delete_model(
    runtime: State<'_, Arc<AppRuntime>>,
    id: String,
) -> Result<serde_json::Value, String> {
    models::delete_model(&runtime.base_url(), &id)
        .await
        .map_err(|error| error.to_string())
}

/// Activating a model applies restart-tier settings, so the shell restarts the
/// sidecar and waits for the next Control connection before returning (same
/// contract as `apply_settings`).
#[tauri::command]
async fn activate_model(
    runtime: State<'_, Arc<AppRuntime>>,
    id: String,
) -> Result<serde_json::Value, String> {
    let result = models::activate_model(&runtime.base_url(), &id)
        .await
        .map_err(|error| error.to_string())?;
    if result.get("tier").and_then(|tier| tier.as_str()) == Some("restart") {
        runtime.restart().await?;
        runtime
            .status
            .wait_until_connected(Duration::from_secs(30))
            .await?;
    }
    Ok(result)
}

#[tauri::command]
async fn get_nemo_services(
    runtime: State<'_, Arc<AppRuntime>>,
) -> Result<serde_json::Value, String> {
    models::nemo_services(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn start_nemo_service(
    runtime: State<'_, Arc<AppRuntime>>,
    id: String,
) -> Result<serde_json::Value, String> {
    models::nemo_start(&runtime.base_url(), &id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn stop_nemo_service(
    runtime: State<'_, Arc<AppRuntime>>,
    id: String,
) -> Result<serde_json::Value, String> {
    models::nemo_stop(&runtime.base_url(), &id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn download_nemo_weights(
    runtime: State<'_, Arc<AppRuntime>>,
    id: String,
) -> Result<serde_json::Value, String> {
    models::nemo_weights(&runtime.base_url(), &id)
        .await
        .map_err(|error| error.to_string())
}

fn setup(app: &mut tauri::App) -> Result<(), String> {
    let control_port = daemon::select_control_port()?;
    let base_url = format!("http://127.0.0.1:{control_port}");
    let status_hub: SharedStatusHub = Arc::new(StatusHub::new());
    let audio_hub: SharedAudioHub = Arc::new(audio::AudioHub::new());
    let model_hub: SharedModelHub = Arc::new(models::ModelHub::new());
    let (restart_tx, restart_rx) = watch::channel(0u64);

    let runtime = Arc::new(AppRuntime::new(
        app.handle().clone(),
        Arc::clone(&status_hub),
        control_port,
        restart_tx,
    ));
    app.manage(Arc::clone(&runtime));
    app.manage(Arc::clone(&audio_hub));
    app.manage(Arc::clone(&model_hub));

    spawn_status_emitter(app.handle().clone(), Arc::clone(&status_hub));
    audio::spawn_audio_level_emitter(app.handle().clone(), Arc::clone(&audio_hub));
    models::spawn_model_progress_emitter(app.handle().clone(), Arc::clone(&model_hub));
    tauri::async_runtime::block_on(runtime.start())?;

    let bridge = ControlBridge::new(
        base_url,
        Arc::clone(&status_hub),
        Arc::clone(&audio_hub),
        Arc::clone(&model_hub),
        restart_rx,
    );
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
        .invoke_handler(tauri::generate_handler![
            desktop_status,
            restart_daemon,
            get_settings,
            validate_settings,
            apply_settings,
            get_audio_devices,
            get_audio_status,
            request_audio_permission,
            tts_preview,
            speaker_test,
            mic_test,
            loopback_test,
            get_models,
            download_model,
            cancel_model_download,
            delete_model,
            activate_model,
            get_nemo_services,
            start_nemo_service,
            stop_nemo_service,
            download_nemo_weights
        ])
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
