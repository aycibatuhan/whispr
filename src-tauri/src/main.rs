// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hotkey;
mod window;
mod audio;
mod config;
mod menu;
mod whisper;
mod postprocess;
mod logging;

use log::{error, warn, info, debug};
use std::sync::{Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tauri::{Manager, App, AppHandle, Wry, Emitter};
use std::time::{Duration, Instant};
use enigo::{Enigo, Keyboard, Settings};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_shell::ShellExt;

use crate::{
    audio::{AudioManager, CaptureOptions},
    window::OverlayWindow,
    hotkey::HotkeyManager,
    config::{ConfigManager, WhisprConfig},
    menu::{create_tray_menu, MenuState},
    whisper::WhisperProcessor,
};

/// Presses shorter than this are treated as accidental taps and discarded.
const MIN_RECORDING_DURATION: Duration = Duration::from_millis(500);
/// How long the "No microphone" overlay stays visible.
const ERROR_OVERLAY_DURATION: Duration = Duration::from_millis(1800);

#[derive(thiserror::Error, Debug)]
#[allow(dead_code)]
pub enum WhisprError {
    #[error("Audio initialization failed: {0}")]
    AudioError(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Hotkey error: {0}")]
    HotkeyError(String),
    #[error("Whisper model error: {0}")]
    WhisperError(String),
    #[error("System error: {0}")]
    SystemError(String),
}

type Result<T> = std::result::Result<T, WhisprError>;

pub struct AppState {
    whisper: WhisperProcessor,
    pub audio: Mutex<AudioManager>,
    overlay: Mutex<OverlayWindow>,
    /// Set from hotkey press until the resulting text has been typed (or the
    /// attempt was abandoned). Enforces a single in-flight dictation and is
    /// reset by `BusyGuard` on *every* exit path, so a failed or empty
    /// dictation can never wedge the hotkey.
    busy: AtomicBool,
    recording_start: Mutex<Option<Instant>>,
    /// Bumped every time the overlay is shown. A delayed hide (used for the
    /// "No microphone" message) only fires if nothing newer was shown since.
    overlay_generation: AtomicU64,
}

/// Lock a mutex even if a previous holder panicked; the protected values
/// here (audio manager, overlay handle) stay usable after a panic.
fn lock_or_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Load settings, falling back to defaults instead of panicking so a corrupt
/// `settings.json` can't take the hotkey down with it.
fn load_config() -> WhisprConfig {
    match ConfigManager::<WhisprConfig>::new("settings").and_then(|m| m.load_config("settings")) {
        Ok(c) => c,
        Err(e) => {
            error!("Could not load settings, using defaults: {}", e);
            WhisprConfig::default()
        }
    }
}

fn config_dir() -> Option<std::path::PathBuf> {
    ConfigManager::<WhisprConfig>::new("settings")
        .ok()
        .map(|m| m.get_config_dir().to_path_buf())
}

fn emit_status(app: &AppHandle, status: &str) {
    let _ = app.emit("status-change", status);
}

impl AppState {
    fn new(config: &WhisprConfig) -> Result<Self> {
        let mut audio_manager = AudioManager::new();
        audio_manager.set_preferred_device(config.audio.device_name.clone());
        audio_manager.set_remove_silence(config.audio.remove_silence);

        let model_path = dirs::home_dir()
            .ok_or_else(|| WhisprError::SystemError("Could not find home directory".to_string()))?
            .join(".whispr")
            .join("model.bin");
        let whisper = WhisperProcessor::new(&model_path, config)
            .map_err(WhisprError::WhisperError)?;

        Ok(Self {
            whisper,
            audio: Mutex::new(audio_manager),
            overlay: Mutex::new(OverlayWindow::new()),
            busy: AtomicBool::new(false),
            recording_start: Mutex::new(None),
            overlay_generation: AtomicU64::new(0),
        })
    }
}

/// Owns the `busy` flag for one dictation attempt. Dropping it (normally, on
/// an early return, or while unwinding from a panic in the worker thread)
/// hides the overlay, reports "Ready" and frees the hotkey again.
struct BusyGuard {
    app: AppHandle,
}

impl BusyGuard {
    fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<AppState>() {
            lock_or_recover(&state.overlay).hide();
            *lock_or_recover(&state.recording_start) = None;
            state.busy.store(false, Ordering::SeqCst);
        }
        emit_status(&self.app, "Ready");
        debug!("Dictation finished; hotkey is free again");
    }
}

fn on_hotkey_pressed(app: &AppHandle) {
    let state = match app.try_state::<AppState>() {
        Some(s) => s,
        None => return,
    };

    if state.busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        debug!("Hotkey pressed while a dictation is still in progress; ignoring");
        return;
    }

    let config = load_config();

    // Get Ollama loading the correction model now, so it is (usually) warm by
    // the time Whisper has finished.
    if config.postprocess.enabled {
        let model = config.postprocess.model.clone();
        std::thread::spawn(move || crate::postprocess::warm_up(&model));
    }

    let options = CaptureOptions {
        preferred_device: config.audio.device_name.clone(),
        save_recording_to: if config.developer.save_recordings {
            config_dir().map(|d| d.join("recordings"))
        } else {
            None
        },
    };

    let started = {
        let mut audio = lock_or_recover(&state.audio);
        audio.set_remove_silence(config.audio.remove_silence);
        audio.start_capture(&options)
    };

    match started {
        Ok(device) => {
            *lock_or_recover(&state.recording_start) = Some(Instant::now());
            state.overlay_generation.fetch_add(1, Ordering::SeqCst);
            lock_or_recover(&state.overlay).show();
            emit_status(app, "Listening");
            debug!("Recording on '{}'", device);
        }
        Err(e) => {
            error!("Cannot start recording: {}", e);
            let generation = state.overlay_generation.fetch_add(1, Ordering::SeqCst) + 1;
            lock_or_recover(&state.overlay).show();
            emit_status(app, "No microphone");
            // Free the hotkey right away (the user may plug in a mic and try
            // again immediately); just keep the message on screen briefly.
            state.busy.store(false, Ordering::SeqCst);
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(ERROR_OVERLAY_DURATION);
                if let Some(state) = app.try_state::<AppState>() {
                    if state.overlay_generation.load(Ordering::SeqCst) == generation
                        && !state.busy.load(Ordering::SeqCst)
                    {
                        lock_or_recover(&state.overlay).hide();
                        emit_status(&app, "Ready");
                    }
                }
            });
        }
    }
}

fn on_hotkey_released(app: &AppHandle) {
    let state = match app.try_state::<AppState>() {
        Some(s) => s,
        None => return,
    };

    // Only act if *this* press actually started a recording. Releases that
    // arrive while transcribing, or after a failed start, are ignored.
    let started_at = match lock_or_recover(&state.recording_start).take() {
        Some(t) => t,
        None => {
            debug!("Hotkey released with no active recording; ignoring");
            return;
        }
    };

    // From here on every exit path goes through the guard.
    let guard = BusyGuard::new(app.clone());

    let captured = {
        let mut audio = lock_or_recover(&state.audio);
        audio.stop_capture();
        let duration = started_at.elapsed();
        if duration < MIN_RECORDING_DURATION {
            debug!("Recording too short ({:.2}s), discarding", duration.as_secs_f32());
            return;
        }
        audio.get_captured_audio(16000, 1)
    };

    let captured = match captured {
        Some(c) => c,
        None => {
            info!("No audio captured");
            return;
        }
    };
    debug!("Got captured audio: {} samples", captured.len());

    emit_status(app, "Transcribing");

    // Whisper + Ollama + typing can take several seconds. Run them off the
    // main thread so the overlay keeps animating, the tray menu stays
    // responsive, and hotkey events are not queued up behind the work.
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("whispr-dictation".into())
        .spawn(move || {
            let _guard = guard;
            run_dictation(&app, captured);
        });
    if let Err(e) = spawned {
        error!("Could not start dictation thread: {}", e);
    }
}

/// Transcribe → (optionally) correct → type. Runs on a worker thread.
fn run_dictation(app: &AppHandle, captured_audio: Vec<f32>) {
    let state = match app.try_state::<AppState>() {
        Some(s) => s,
        None => return,
    };
    let config = load_config();

    let segments = match state.whisper.process_audio(&captured_audio, &config.whisper) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to process audio: {}", e);
            return;
        }
    };

    let mut transcription: String = segments
        .iter()
        .map(|(_, _, segment)| segment.as_str())
        .collect::<Vec<&str>>()
        .join(" ");
    if transcription.trim().is_empty() {
        info!("No transcription segments produced");
        return;
    }
    // Trailing space after punctuation lets consecutive dictations chain.
    if transcription.chars().last().map_or(false, |c| c.is_ascii_punctuation()) {
        transcription.push(' ');
    }
    info!("Transcription: {}", transcription);

    if config.postprocess.enabled {
        emit_status(app, "Correcting");
        match crate::postprocess::correct(&transcription, &config.postprocess) {
            Ok(corrected) => transcription = corrected,
            Err(e) => error!("Post-processing failed, inserting raw transcription: {}", e),
        }
    }

    let mut enigo = match Enigo::new(&Settings::default()) {
        Ok(enigo) => enigo,
        Err(e) => {
            error!(
                "Failed to create Enigo instance (is Accessibility permission granted for Whispr?): {}",
                e
            );
            return;
        }
    };

    if let Err(e) = enigo.text(&transcription) {
        error!("Failed to send text: {}", e);
    }
}

fn setup_app(app: &mut App<Wry>) -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Menu-bar app: no Dock icon, and Whispr never becomes the frontmost app
    // (not even at launch). That keeps keyboard focus in the app the user is
    // dictating into, so the typed text lands there and not in the overlay.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let app_handle = app.handle();

    let config_manager = ConfigManager::<WhisprConfig>::new("settings")
        .map_err(|e| WhisprError::ConfigError(e.to_string()))?;

    let model_path = config_manager.get_config_dir().join("model.bin");
    if !model_path.exists() {
        app.dialog()
            .message("Model file not found at ~/.whispr/model.bin - see README.md")
            .kind(MessageDialogKind::Error)
            .title("Error")
            .blocking_show();

        let _ = app.shell().command("open")
            .args(["https://github.com/dbpprt/whispr?tab=readme-ov-file#usage"])
            .spawn();

        app.handle().exit(1);
        return Ok(());
    }

    let mut whispr_config = load_config();

    // Remember the first microphone we see if none is configured yet. If no
    // microphone is connected right now this is simply skipped; capture
    // resolves a device on demand anyway.
    if whispr_config.audio.device_name.is_none() {
        if let Some(first_device) = AudioManager::new().list_input_devices().into_iter().next() {
            whispr_config.audio.device_name = Some(first_device);
            if let Err(e) = config_manager.save_config(&whispr_config, "settings") {
                error!("Failed to save default audio device: {}", e);
            }
        } else {
            warn!("No microphone connected at startup; will look again when the hotkey is pressed");
        }
    }

    // Initialize Enigo once to prompt for the Accessibility permission early.
    match Enigo::new(&Settings::default()) {
        Ok(_) => info!("Successfully initialized Enigo"),
        Err(e) => warn!("Failed to initialize Enigo: {}", e),
    }

    let state = AppState::new(&whispr_config)?;
    lock_or_recover(&state.overlay).create_window(app_handle);
    app.manage(state);

    let (tray_menu, menu_state) = create_tray_menu(app_handle);
    app.manage(menu_state);

    let handle_clone = app.handle().clone();
    let tray = tauri::tray::TrayIconBuilder::new()
        .icon(app_handle.default_window_icon().unwrap().clone())
        .show_menu_on_left_click(false)
        .menu(&tray_menu)
        .on_menu_event(move |app, event| {
            let menu_state = handle_clone.state::<MenuState<_>>();
            crate::menu::handle_menu_event(app.clone(), &event.id().0, &menu_state);
        })
        .build(app.handle())
        .map_err(|e| Box::new(WhisprError::SystemError(e.to_string())) as Box<dyn std::error::Error>)?;

    app.manage(tray);

    let hotkey_handle = app.handle().clone();
    let mut hotkey_manager = HotkeyManager::new(move |is_pressed| {
        if is_pressed {
            on_hotkey_pressed(&hotkey_handle);
        } else {
            on_hotkey_released(&hotkey_handle);
        }
    }, whispr_config.clone());

    if let Err(e) = hotkey_manager.start() {
        error!("Failed to start hotkey manager: {}", e);
    }
    // The NSEvent monitors live as long as the process; keep the manager
    // alive alongside them rather than dropping it at the end of setup.
    app.manage(Mutex::new(hotkey_manager));

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn main() {
    if let Err(e) = logging::setup_logging() {
        eprintln!("Failed to initialize logging: {}", e);
    }

    info!("Starting Whispr application");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            info!("{}, {argv:?}, {cwd}", app.package_info().name);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .setup(setup_app)
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
