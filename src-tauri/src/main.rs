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
use std::sync::{Arc, Mutex};
use tauri::{Manager, App, Wry, Emitter};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use enigo::{Enigo, Keyboard, Settings};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_shell::ShellExt;

use crate::{
    audio::AudioManager,
    window::OverlayWindow,
    hotkey::HotkeyManager,
    config::{ConfigManager, WhisprConfig},
    menu::{create_tray_menu, MenuState},
    whisper::WhisperProcessor,
};
const MIN_RECORDING_DURATION: Duration = Duration::from_secs(1);

#[derive(thiserror::Error, Debug)]
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

struct AppState {
    whisper: WhisperProcessor,
    audio: Mutex<Option<AudioManager>>,
    overlay: Mutex<OverlayWindow>,
    recording_semaphore: Arc<Semaphore>,
    recording_start: Mutex<Option<Instant>>,
}

impl AppState {
    fn new(config: WhisprConfig) -> Result<Self> {
        let audio_manager = match AudioManager::new() {
            Ok(am) => Some(am),
            Err(e) => {
                warn!("No audio input device available at startup ({}). Will retry on hotkey press.", e);
                None
            }
        };
        
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
            recording_semaphore: Arc::new(Semaphore::new(1)),
            recording_start: Mutex::new(None),
        })
    }

    fn configure_audio(&self, config: &WhisprConfig) -> Result<()> {
        let mut audio = self.audio.lock().unwrap();
        if let Some(manager) = audio.as_mut() {
            if let Some(device_name) = &config.audio.device_name {
                manager.set_input_device(device_name)
                    .map_err(|e| WhisprError::AudioError(e.to_string()))?;
            }
            manager.set_remove_silence(config.audio.remove_silence);
        }
        Ok(())
    }
}

fn setup_app(app: &mut App<Wry>) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let app_handle = app.handle();
    
    // Initialize configuration
    let config_manager = ConfigManager::<WhisprConfig>::new("settings")
        .map_err(|e| WhisprError::ConfigError(e.to_string()))?;
    
    // Check if model file exists
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
    
    let mut whispr_config = if config_manager.config_exists("settings") {
        config_manager.load_config("settings")
            .map_err(|e| WhisprError::ConfigError(e.to_string()))?
    } else {
        WhisprConfig::default()
    };

    // Set default audio device if none is configured (tolerant: no mic = skip)
    if whispr_config.audio.device_name.is_none() {
        if let Ok(temp_audio) = AudioManager::new() {
            if let Ok(devices) = temp_audio.list_input_devices() {
                if let Some(first_device) = devices.first() {
                    whispr_config.audio.device_name = Some(first_device.clone());
                    if let Err(e) = config_manager.save_config(&whispr_config, "settings") {
                        error!("Failed to save default audio device: {}", e);
                    }
                }
            }
        }
    }

    // Initialize Enigo once to prompt for permissions
    match Enigo::new(&Settings::default()) {
        Ok(_) => info!("Successfully initialized Enigo"),
        Err(e) => warn!("Failed to initialize Enigo: {}", e),
    }

    // Initialize application state
    let state = AppState::new(whispr_config.clone())?;
    state.configure_audio(&whispr_config)?;
    
    // Create window
    state.overlay.lock().unwrap().create_window(app_handle);
    
    // Store state
    app.manage(state);

    // Setup tray and menu
    let (tray_menu, menu_state) = create_tray_menu(app_handle);
    app.manage(menu_state);

    let handle_clone = app.handle().clone();
    let tray = tauri::tray::TrayIconBuilder::new()
        .icon(app_handle.default_window_icon().unwrap().clone())
        .menu_on_left_click(false)
        .menu(&tray_menu)
        .on_menu_event(move |app, event| {
            let menu_state = handle_clone.state::<MenuState<_>>();
            crate::menu::handle_menu_event(app.clone(), &event.id().0, &menu_state);
        })
        .build(app.handle())
        .map_err(|e| Box::new(WhisprError::SystemError(e.to_string())) as Box<dyn std::error::Error>)?;
    
    app.manage(tray);

    // Setup hotkey manager
    let app_handle_clone = app.handle().clone();
    let postprocess_settings = whispr_config.postprocess.clone();
    let mut hotkey_manager = HotkeyManager::new(move |is_speaking| {
        if let Some(state) = app_handle_clone.try_state::<AppState>() {
            let overlay = state.overlay.lock().unwrap();
            
            if is_speaking {
                // Try to acquire the semaphore permit
                if let Ok(_permit) = state.recording_semaphore.try_acquire() {
                    // Keep the permit held until the explicit add_permits(1)
                    // on stop/failure paths; without forget() the RAII guard
                    // would release it AND add_permits(1) would grow the pool.
                    _permit.forget();
                    overlay.show();
                    let mut audio = state.audio.lock().unwrap();
                    // Lazily (re)create the audio manager in case no input device was
                    // available at startup (e.g. AirPods not connected yet).
                    if audio.is_none() {
                        match AudioManager::new() {
                            Ok(mut am) => {
                                // Apply the user's saved device/silence settings,
                                // same as configure_audio does at startup.
                                if let Ok(config) = ConfigManager::<WhisprConfig>::new("settings")
                                    .and_then(|m| m.load_config("settings"))
                                {
                                    if let Some(device_name) = &config.audio.device_name {
                                        let _ = am.set_input_device(device_name);
                                    }
                                    am.set_remove_silence(config.audio.remove_silence);
                                }
                                *audio = Some(am);
                            }
                            Err(e) => {
                                error!("No audio input device available: {}", e);
                                overlay.hide();
                                state.recording_semaphore.add_permits(1);
                                return;
                            }
                        }
                    }
                    if let Some(manager) = audio.as_mut() {
                        if let Err(e) = manager.start_capture() {
                            error!("Failed to start audio capture: {}", e);
                            overlay.hide();
                            state.recording_semaphore.add_permits(1);
                            return;
                        }
                    }
                    *state.recording_start.lock().unwrap() = Some(Instant::now());
                    let _ = app_handle_clone.emit("status-change", "Listening");
                } else {
                    warn!("Recording already in progress");
                }
            } else {
                let captured_audio = {
                    let mut audio = state.audio.lock().unwrap();
                    let manager = match audio.as_mut() {
                        Some(m) => m,
                        None => {
                            warn!("No audio manager (nothing was recording)");
                            return;
                        }
                    };
                    manager.stop_capture();
                    
                    // Check recording duration
                    if let Some(start_time) = state.recording_start.lock().unwrap().take() {
                        let duration = start_time.elapsed();
                        if duration < MIN_RECORDING_DURATION {
                            debug!("Recording too short ({:.2}s), discarding", duration.as_secs_f32());
                            let _ = app_handle_clone.emit("status-change", "Ready");
                            overlay.hide();
                            return;
                        }
                    }
                    
                    let _ = app_handle_clone.emit("status-change", "Transcribing");
                    manager.get_captured_audio(16000, 1)
                };
                
                if let Some(captured_audio) = captured_audio {
                    debug!("Got captured audio: {} samples", captured_audio.len());
                    
                    match state.whisper.process_audio(captured_audio) {
                        Ok(segments) => {
                            if segments.is_empty() {
                                info!("No transcription segments produced");
                                let _ = app_handle_clone.emit("status-change", "Ready");
                                overlay.hide();
                                return;
                            }
                            
                            let mut transcription: String = segments.iter()
                                .map(|(_, _, segment)| segment.clone())
                                .collect::<Vec<String>>()
                                .join(" ");
                            // Add trailing space if last character is punctuation, allowing for "chaining" of recordings
                            if let Some(last_char) = transcription.chars().last() {
                                if last_char.is_ascii_punctuation() {
                                    transcription.push(' ');
                                }
                            }
                            info!("Transcription: {}", transcription);

                            // Optional local LLM post-processing via Ollama.
                            // Reload settings at dictation time so tray-menu
                            // toggles take effect without an app restart.
                            let postprocess_settings = ConfigManager::<WhisprConfig>::new("settings")
                                .ok()
                                .and_then(|m| m.load_config("settings").ok())
                                .map(|c| c.postprocess)
                                .unwrap_or_else(|| postprocess_settings.clone());
                            if postprocess_settings.enabled {
                                let _ = app_handle_clone.emit("status-change", "Correcting");
                                match crate::postprocess::correct(&transcription, &postprocess_settings) {
                                    Ok(corrected) => transcription = corrected,
                                    Err(e) => {
                                        error!("Post-processing failed, inserting raw transcription: {}", e);
                                        let _ = app_handle_clone.emit("status-change", "Ready");
                                    }
                                }
                            }

                            // Create a new Enigo instance for text input
                            let mut enigo = match Enigo::new(&Settings::default()) {
                                Ok(enigo) => enigo,
                                Err(e) => {
                                    error!("Failed to create Enigo instance: {}", e);
                                    let _ = app_handle_clone.emit("status-change", "Ready");
                                    overlay.hide();
                                    return;
                                }
                            };
                            
                            if let Err(e) = enigo.text(&transcription) {
                                error!("Failed to send text: {}", e);
                                let _ = app_handle_clone.emit("status-change", "Ready");
                                overlay.hide();
                                return;
                            }
                            
                            let _ = app_handle_clone.emit("status-change", "Ready");
                        }
                        Err(e) => {
                            error!("Failed to process audio: {}", e);
                            let _ = app_handle_clone.emit("status-change", "Ready");
                            overlay.hide();
                            return;
                        }
                    }
                } else {
                    info!("No audio captured");
                    let _ = app_handle_clone.emit("status-change", "Ready");
                    overlay.hide();
                    return;
                }
                
                overlay.hide();
                
                // Release the semaphore permit
                state.recording_semaphore.add_permits(1);
            }
        }
    }, whispr_config.clone());

    if let Err(e) = hotkey_manager.start() {
        error!("Failed to start hotkey manager: {}", e);
    }

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
        .plugin(tauri_plugin_process::init())  // Register the process plugin
        .setup(setup_app)
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
