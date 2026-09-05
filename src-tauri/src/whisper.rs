use whisper_rs::{WhisperContext, WhisperContextParameters, FullParams, SamplingStrategy};
use crate::config::{WhisperSettings, WhisprConfig};
use log::info;
use std::sync::Arc;
use std::result::Result;

pub struct WhisperProcessor {
    ctx: Arc<WhisperContext>,
}

unsafe extern "C" fn whisper_cpp_log_trampoline(
    _: u32, // ggml_log_level
    _: *const std::os::raw::c_char,
    _: *mut std::os::raw::c_void, // user_data
) { }

impl WhisperProcessor {
    pub fn new(model_path: &std::path::Path, config: &WhisprConfig) -> Result<Self, String> {
        if !config.developer.whisper_logging {
            unsafe {
                whisper_rs::set_log_callback(Some(whisper_cpp_log_trampoline), std::ptr::null_mut());
            }
        }

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or_else(|| "Invalid model path".to_string())?,
            WhisperContextParameters::default()
        ).map_err(|e| e.to_string())?;

        Ok(Self {
            ctx: Arc::new(ctx),
        })
    }

    /// Transcribe 16 kHz mono audio. `settings` is passed per call so that
    /// language / translate / dictionary changes made in the tray menu apply
    /// to the next dictation without restarting the app.
    pub fn process_audio(&self, captured_audio: &[f32], settings: &WhisperSettings) -> Result<Vec<(f32, f32, String)>, String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        // "auto" (from the tray menu) means language detection, which whisper
        // expects as `None` rather than the literal string.
        let language = settings.language.as_deref().filter(|l| *l != "auto");
        params.set_language(language);
        params.set_translate(settings.translate);
        if let Some(dict) = &settings.dictionary {
            if !dict.is_empty() {
                let prompt = format!("This audio uses specialized terms including: {}. Please use their exact writing.", dict.join(", "));
                info!("Prompt based on dict: {}", &prompt);
                params.set_initial_prompt(&prompt);
            }
        }

        let mut state = self.ctx.create_state()
            .map_err(|e| e.to_string())?;

        state.full(params, captured_audio)
            .map_err(|e| e.to_string())?;

        let num_segments = state.full_n_segments()
            .map_err(|e| e.to_string())?;

        let mut segments = Vec::new();
        for i in 0..num_segments {
            let segment = state.full_get_segment_text(i)
                .map_err(|e| e.to_string())?.trim().into();
            let start = state.full_get_segment_t0(i)
                .map_err(|e| e.to_string())? as f32;
            let end = state.full_get_segment_t1(i)
                .map_err(|e| e.to_string())? as f32;

            info!("[{} - {}]: \"{}\"", start, end, segment);
            segments.push((start, end, segment));
        }
        Ok(segments)
    }
}
