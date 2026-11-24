mod audio;
mod chat;
mod control;
mod daemon;
mod models;
mod onboarding;
mod settings;
mod status;
mod update;

use std::sync::Arc;
use std::time::Duration;

use tauri::{Manager, State};
use tokio::sync::watch;

use crate::audio::SharedAudioHub;
use crate::chat::SharedChatHub;
use crate::control::ControlBridge;
use crate::daemon::{AppRuntime, spawn_status_emitter};
use crate::models::SharedModelHub;
use crate::onboarding::OnboardingStore;
use crate::status::{SharedStatusHub, StatusHub};
use crate::update::PendingUpdate;

#[tauri::command]
async fn desktop_status(runtime: State<'_, Arc<AppRuntime>>) -> Result<status::AppStatus, String> {
    Ok(runtime.status.snapshot())
}

#[tauri::command]
fn get_onboarding_state(
    store: State<'_, Arc<OnboardingStore>>,
) -> Result<onboarding::OnboardingState, String> {
    Ok(store.load())
}

#[tauri::command]
fn begin_onboarding(
    store: State<'_, Arc<OnboardingStore>>,
    runtime: State<'_, Arc<AppRuntime>>,
) -> Result<onboarding::OnboardingState, String> {
    let mut state = store.load();
    // A re-run starts a fresh skip record; resuming an unfinished first run
    // keeps the existing one.
    if state.completed {
        state.skipped_steps.clear();
    }
    state.in_progress = true;
    state.current_step = Some(onboarding::ONBOARDING_STEPS[0].to_string());
    store.save(&state)?;
    runtime.set_onboarding_active(true);
    Ok(state)
}

#[tauri::command]
fn record_onboarding_step(
    store: State<'_, Arc<OnboardingStore>>,
    step: String,
    skipped: bool,
) -> Result<onboarding::OnboardingState, String> {
    if !onboarding::ONBOARDING_STEPS.contains(&step.as_str()) {
        return Err(format!("unknown onboarding step '{step}'"));
    }
    let mut state = store.load();
    state.in_progress = true;
    state.current_step = Some(step.clone());
    if skipped {
        if !state.skipped_steps.contains(&step) {
            state.skipped_steps.push(step);
        }
    } else {
        state.skipped_steps.retain(|candidate| candidate != &step);
    }
    store.save(&state)?;
    Ok(state)
}

#[tauri::command]
fn abandon_onboarding(
    store: State<'_, Arc<OnboardingStore>>,
    runtime: State<'_, Arc<AppRuntime>>,
) -> Result<onboarding::OnboardingState, String> {
    let mut state = store.load();
    state.in_progress = false;
    state.current_step = None;
    store.save(&state)?;
    runtime.set_onboarding_active(false);
    Ok(state)
}

/// Marks the durable marker only after the final restart and audible greeting.
#[tauri::command]
fn complete_onboarding(
    store: State<'_, Arc<OnboardingStore>>,
    runtime: State<'_, Arc<AppRuntime>>,
) -> Result<onboarding::OnboardingState, String> {
    let mut state = store.load();
    state.completed = true;
    state.in_progress = false;
    state.current_step = None;
    store.save(&state)?;
    runtime.set_onboarding_active(false);
    Ok(state)
}

#[tauri::command]
async fn restart_daemon(runtime: State<'_, Arc<AppRuntime>>) -> Result<(), String> {
    runtime.restart().await
}

#[tauri::command]
fn chat_snapshot(chat: State<'_, SharedChatHub>) -> Result<chat::ChatSnapshot, String> {
    Ok(chat.snapshot())
}

#[tauri::command]
async fn refresh_chat_history(
    runtime: State<'_, Arc<AppRuntime>>,
    chat: State<'_, SharedChatHub>,
) -> Result<chat::ChatSnapshot, String> {
    let entries = chat::fetch_history(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())?;
    chat.replace_history(entries);
    Ok(chat.snapshot())
}

#[tauri::command]
async fn send_chat_input(
    runtime: State<'_, Arc<AppRuntime>>,
    text: String,
) -> Result<(), String> {
    chat::send_text_input(&runtime.base_url(), &text)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn get_onboarding_defaults(
    runtime: State<'_, Arc<AppRuntime>>,
) -> Result<serde_json::Value, String> {
    settings::get_onboarding_defaults(&runtime.base_url())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_onboarding_greeting(
    runtime: State<'_, Arc<AppRuntime>>,
    active: bool,
) -> Result<(), String> {
    runtime.set_onboarding_active(active);
    Ok(())
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
    let app_data_dir = app
        .path()
        .app_data_dir()
        .ok()
        .filter(|path| !path.as_os_str().is_empty());
    let onboarding_path = app
        .path()
        .app_config_dir()
        .map(|dir| dir.join("onboarding.json"))
        .map_err(|error| format!("failed to resolve the onboarding path: {error}"))?;
    let store = Arc::new(OnboardingStore::new(onboarding_path));

    let workdir = daemon::daemon_workdir(app_data_dir);
    let config_path = daemon::daemon_config_path(&workdir);
    let espeak_data_root = app.path().resource_dir().ok();
    let mut onboarding_state = store.load();
    let config_exists = config_path.exists();
    if !config_exists {
        // Always give the daemon a runnable file at the explicit path; the
        // wizard then decides whether this is first run or a repair.
        onboarding::write_bootstrap_config(&config_path)?;
    }
    let wizard_active = onboarding::should_open_wizard(&onboarding_state, config_exists);
    if wizard_active {
        onboarding_state.in_progress = true;
        if onboarding_state.current_step.is_none() {
            onboarding_state.current_step = Some(onboarding::ONBOARDING_STEPS[0].to_string());
        }
        store.save(&onboarding_state)?;
    }

    let status_hub: SharedStatusHub = Arc::new(StatusHub::new());
    let audio_hub: SharedAudioHub = Arc::new(audio::AudioHub::new());
    let model_hub: SharedModelHub = Arc::new(models::ModelHub::new());
    let chat_hub: SharedChatHub = Arc::new(chat::ChatHub::new());
    let (restart_tx, restart_rx) = watch::channel(0u64);

    let runtime = Arc::new(
        AppRuntime::new(
            app.handle().clone(),
            Arc::clone(&status_hub),
            control_port,
            restart_tx,
            workdir,
            config_path,
            wizard_active,
        )
        .with_resource_root(espeak_data_root),
    );
    app.manage(Arc::clone(&runtime));
    app.manage(Arc::clone(&audio_hub));
    app.manage(Arc::clone(&model_hub));
    app.manage(Arc::clone(&chat_hub));
    app.manage(Arc::clone(&store));
    app.manage(PendingUpdate::new());

    spawn_status_emitter(app.handle().clone(), Arc::clone(&status_hub));
    audio::spawn_audio_level_emitter(app.handle().clone(), Arc::clone(&audio_hub));
    models::spawn_model_progress_emitter(app.handle().clone(), Arc::clone(&model_hub));
    chat::spawn_chat_event_emitter(app.handle().clone(), Arc::clone(&chat_hub));
    tauri::async_runtime::block_on(runtime.start())?;

    let bridge = ControlBridge::new(
        base_url,
        Arc::clone(&status_hub),
        Arc::clone(&audio_hub),
        Arc::clone(&model_hub),
        Arc::clone(&chat_hub),
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
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            desktop_status,
            restart_daemon,
            chat_snapshot,
            refresh_chat_history,
            send_chat_input,
            get_onboarding_state,
            begin_onboarding,
            record_onboarding_step,
            abandon_onboarding,
            complete_onboarding,
            get_onboarding_defaults,
            set_onboarding_greeting,
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
            download_nemo_weights,
            update::check_for_update,
            update::install_update
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
