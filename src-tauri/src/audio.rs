use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use log::{error, warn, info, debug};
use cpal::{Device, Host, Stream, StreamConfig};
use hound::{WavWriter, WavSpec};
use std::sync::{Arc, Mutex};
use std::fs::File;
use std::io::BufWriter;
use chrono::Local;
use anyhow::Error;
use std::collections::VecDeque;
use samplerate::{convert, ConverterType};
use std::time::Instant;

fn audio_resample(data: &[f32], sample_rate0: u32, sample_rate: u32, channels: u16) -> Vec<f32> {
    convert(
        sample_rate0 as _,
        sample_rate as _,
        channels as _,
        ConverterType::SincBestQuality,
        data,
    ).unwrap_or_default()
}

fn stereo_to_mono(stereo_data: &[f32]) -> Vec<f32> {
    let mut mono_data = Vec::with_capacity(stereo_data.len() / 2);
    for chunk in stereo_data.chunks_exact(2) {
        let average = (chunk[0] + chunk[1]) / 2.0;
        mono_data.push(average);
    }
    mono_data
}

#[derive(Clone)]
pub struct SilenceConfig {
    enabled: bool,
    threshold: f32,
    min_silence_duration: usize,
}

impl Default for SilenceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 0.01,
            min_silence_duration: 1000,
        }
    }
}

/// Options that apply to a single capture. Loaded from settings by the caller
/// at hotkey-press time so tray-menu changes take effect without a restart.
#[derive(Clone, Debug, Default)]
pub struct CaptureOptions {
    /// Preferred input device by name. Falls back to the system default (or
    /// any available input) when this device is not connected right now.
    pub preferred_device: Option<String>,
    /// Write the raw capture to `<dir>/<timestamp>.wav`.
    pub save_recording_to: Option<std::path::PathBuf>,
}

/// Manages microphone capture.
///
/// Unlike a naive design that grabs a `cpal::Device` once at startup, this
/// resolves the input device *every time a capture starts*. That matters on
/// machines whose only microphone is a Bluetooth headset (e.g. a Mac mini with
/// AirPods): the device comes and goes, and a cached handle turns into a
/// permanent "unknown CoreAudio error" the moment it disconnects.
pub struct AudioManager {
    host: Host,
    preferred_device_name: Option<String>,
    /// Name of the device used by the most recent (or current) capture.
    active_device_name: Option<String>,
    stream: Option<Stream>,
    is_capturing: Arc<Mutex<bool>>,
    wav_writer: Arc<Mutex<Option<WavWriter<BufWriter<File>>>>>,
    silence_config: Arc<Mutex<SilenceConfig>>,
    start_time: Arc<Mutex<Option<Instant>>>,
    captured_audio: Arc<Mutex<VecDeque<f32>>>,
    /// (sample_rate, channels) of the data currently in `captured_audio`.
    /// Recorded at capture start so we never have to ask a device that may
    /// have disappeared in the meantime.
    captured_format: Option<(u32, u16)>,
}

// `cpal::Stream` is !Send on macOS. All stream start/stop happens on the main
// thread (the NSEvent hotkey monitors run there); other threads only ever
// touch the Arc<Mutex<..>> buffers. Keeping the manager inside a Mutex in the
// Tauri state requires these impls.
unsafe impl Send for AudioManager {}
unsafe impl Sync for AudioManager {}

impl AudioManager {
    /// Creating a manager never fails, even with no microphone attached. The
    /// device is resolved lazily in `start_capture`.
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
            preferred_device_name: None,
            active_device_name: None,
            stream: None,
            is_capturing: Arc::new(Mutex::new(false)),
            wav_writer: Arc::new(Mutex::new(None)),
            silence_config: Arc::new(Mutex::new(SilenceConfig::default())),
            start_time: Arc::new(Mutex::new(None)),
            captured_audio: Arc::new(Mutex::new(VecDeque::new())),
            captured_format: None,
        }
    }

    /// Remember the user's preferred device. Does not require it to be
    /// connected right now.
    pub fn set_preferred_device(&mut self, device_name: Option<String>) {
        self.preferred_device_name = device_name;
    }

    /// Select an input device by name. Fails if that device is not currently
    /// available; the previous preference is kept in that case.
    pub fn set_input_device(&mut self, device_name: &str) -> Result<(), Error> {
        if self.list_input_devices().iter().any(|n| n == device_name) {
            self.preferred_device_name = Some(device_name.to_string());
            Ok(())
        } else {
            Err(anyhow::anyhow!("Device not found: {}", device_name))
        }
    }

    /// The device used by the last capture, or the preferred one if nothing
    /// has been captured yet.
    pub fn get_current_device_name(&self) -> Option<String> {
        self.active_device_name
            .clone()
            .or_else(|| self.preferred_device_name.clone())
    }

    pub fn configure_silence_removal(&self, enabled: bool, threshold: Option<f32>, min_silence_duration: Option<usize>) {
        let mut config = self.silence_config.lock().unwrap();
        config.enabled = enabled;
        if let Some(t) = threshold {
            config.threshold = t;
        }
        if let Some(d) = min_silence_duration {
            config.min_silence_duration = d;
        }
    }

    pub fn is_silence_removal_enabled(&self) -> bool {
        self.silence_config.lock().unwrap().enabled
    }

    pub fn set_remove_silence(&mut self, remove_silence: bool) {
        self.configure_silence_removal(remove_silence, None, None);
    }

    /// Names of all input devices that are usable right now.
    pub fn list_input_devices(&self) -> Vec<String> {
        self.usable_input_devices()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Enumerate input devices, keeping only those that report a name and a
    /// default input format.
    ///
    /// Deliberately uses `host.devices()` + `default_input_config()` (both
    /// plain CoreAudio property reads) instead of `host.input_devices()`. The
    /// latter probes every device by opening an AudioUnit on it, which on this
    /// kind of setup (HDMI/DisplayPort monitor audio, Bluetooth headsets) has
    /// been observed to block for close to a minute inside coreaudiod, and it
    /// runs on the main thread at hotkey time.
    ///
    /// Also, when no microphone exists CoreAudio hands out a "default input
    /// device" with an invalid id whose queries fail with an opaque "unknown
    /// error"; validating every candidate here avoids ever touching it.
    fn usable_input_devices(&self) -> Vec<(String, Device)> {
        let devices = match self.host.devices() {
            Ok(d) => d,
            Err(e) => {
                warn!("Could not enumerate audio devices: {}", e);
                return Vec::new();
            }
        };
        devices
            .filter_map(|d| {
                let name = d.name().ok()?;
                if d.default_input_config().is_err() {
                    // Output-only device (speakers, monitor audio, ...).
                    return None;
                }
                Some((name, d))
            })
            .collect()
    }

    /// Pick the device for a new capture: preferred → system default → any.
    fn resolve_input_device(&self, preferred: Option<&str>) -> Result<(String, Device), Error> {
        let mut usable = self.usable_input_devices();
        if usable.is_empty() {
            return Err(anyhow::anyhow!("No microphone connected"));
        }

        if let Some(pref) = preferred {
            if let Some(idx) = usable.iter().position(|(n, _)| n == pref) {
                return Ok(usable.swap_remove(idx));
            }
            let available: Vec<&str> = usable.iter().map(|(n, _)| n.as_str()).collect();
            warn!(
                "Configured input device '{}' is not connected; falling back (available: {:?})",
                pref, available
            );
        }

        if let Some(default) = self.host.default_input_device() {
            if let Ok(default_name) = default.name() {
                if let Some(idx) = usable.iter().position(|(n, _)| *n == default_name) {
                    return Ok(usable.swap_remove(idx));
                }
            }
        }

        Ok(usable.swap_remove(0))
    }

    /// Start capturing. Returns the name of the device actually used.
    pub fn start_capture(&mut self, options: &CaptureOptions) -> Result<String, Error> {
        // Never stack streams: if a previous capture was left open for any
        // reason, tear it down first.
        if self.stream.is_some() {
            warn!("start_capture called while a stream was still open; stopping it first");
            self.stop_capture();
        }

        let preferred = options
            .preferred_device
            .as_deref()
            .or(self.preferred_device_name.as_deref());
        let (device_name, device) = self.resolve_input_device(preferred)?;
        info!("Using input device: {}", device_name);

        let default_config = device.default_input_config()?;
        debug!("Default input config: {:?}", default_config);

        let config = StreamConfig {
            channels: default_config.channels(),
            sample_rate: default_config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        };
        debug!("Using input config: {:?}", config);

        let writer = options.save_recording_to.as_ref().and_then(|recordings_dir| {
            let spec = WavSpec {
                channels: config.channels,
                sample_rate: config.sample_rate.0,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
            let file_path = recordings_dir.join(format!("{}.wav", timestamp));
            if let Err(e) = std::fs::create_dir_all(recordings_dir) {
                error!("Could not create recordings directory {}: {}", recordings_dir.display(), e);
                return None;
            }
            match WavWriter::create(&file_path, spec) {
                Ok(w) => {
                    info!("Saving recording to: {}", file_path.display());
                    Some(w)
                }
                Err(e) => {
                    // A failed debug recording must never block dictation.
                    error!("Could not create recording file {}: {}", file_path.display(), e);
                    None
                }
            }
        });

        *self.wav_writer.lock().unwrap() = writer;
        *self.start_time.lock().unwrap() = Some(Instant::now());
        self.captured_audio.lock().unwrap().clear();
        self.captured_format = Some((config.sample_rate.0, config.channels));

        let stream = self.build_input_stream_f32(
            &device,
            &config,
            self.is_capturing.clone(),
            self.wav_writer.clone(),
            self.silence_config.clone(),
            self.captured_audio.clone(),
        )?;

        stream.play()?;
        self.stream = Some(stream);
        *self.is_capturing.lock().unwrap() = true;
        self.active_device_name = Some(device_name.clone());

        info!("Capture started");
        Ok(device_name)
    }

    pub fn stop_capture(&mut self) {
        // Nothing to do for a manager that never started (e.g. the throwaway
        // instance used to list devices for the tray menu).
        if self.stream.is_none() && self.start_time.lock().unwrap().is_none() {
            return;
        }

        // First mark as not capturing to prevent any new data from being processed
        *self.is_capturing.lock().unwrap() = false;

        if let Some(stream) = self.stream.take() {
            if let Err(e) = stream.pause() {
                error!("Error pausing stream: {}", e);
            }
            drop(stream);
        }

        if let Some(writer) = self.wav_writer.lock().unwrap().take() {
            if let Err(e) = writer.finalize() {
                error!("Error finalizing WAV file: {}", e);
            }
        }

        if let Some(start_time) = self.start_time.lock().unwrap().take() {
            info!("Recording stopped after: {:.2}s", start_time.elapsed().as_secs_f32());
        }

        // Give the audio thread a moment to flush its last buffer.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let samples = self.captured_audio.lock().unwrap().len();
        debug!("Audio buffer contains {} samples", samples);
    }

    fn build_input_stream_f32(
        &self,
        device: &Device,
        config: &StreamConfig,
        is_capturing: Arc<Mutex<bool>>,
        wav_writer: Arc<Mutex<Option<WavWriter<BufWriter<File>>>>>,
        silence_config: Arc<Mutex<SilenceConfig>>,
        captured_audio: Arc<Mutex<VecDeque<f32>>>,
    ) -> Result<Stream, Error> {
        let mut silence_counter = 0usize;
        let mut is_in_silence = false;

        let input_data_fn = move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if !*is_capturing.lock().unwrap() {
                return;
            }

            let (is_silence_enabled, silence_threshold, min_silence_duration) = {
                let cfg = silence_config.lock().unwrap();
                (cfg.enabled, cfg.threshold, cfg.min_silence_duration)
            };

            let mut samples_to_keep = Vec::with_capacity(data.len());

            if is_silence_enabled {
                for &sample in data {
                    let amplitude = sample.abs();
                    if amplitude > silence_threshold {
                        if is_in_silence {
                            silence_counter = 0;
                            is_in_silence = false;
                        }
                        samples_to_keep.push(sample);
                    } else if !is_in_silence {
                        silence_counter += 1;
                        if silence_counter >= min_silence_duration {
                            is_in_silence = true;
                        } else {
                            samples_to_keep.push(sample);
                        }
                    }
                }
            } else {
                samples_to_keep.extend_from_slice(data);
            }

            {
                let mut writer_guard = wav_writer.lock().unwrap();
                if let Some(ref mut writer) = *writer_guard {
                    for &sample in &samples_to_keep {
                        writer.write_sample(sample).unwrap_or_else(|e| error!("Error writing sample: {}", e));
                    }
                }
            }

            captured_audio.lock().unwrap().extend(samples_to_keep);
        };

        let stream = device.build_input_stream(
            config,
            input_data_fn,
            move |err| error!("An error occurred on the audio stream: {}", err),
            None,
        )?;

        Ok(stream)
    }

    /// Drain the captured buffer, converted to the requested format.
    pub fn get_captured_audio(&mut self, desired_sample_rate: u32, desired_channels: u16) -> Option<Vec<f32>> {
        let audio_data: Vec<f32> = {
            let mut audio_buffer = self.captured_audio.lock().unwrap();
            if audio_buffer.is_empty() {
                debug!("Audio buffer is empty");
                return None;
            }
            debug!("Processing {} samples from audio buffer", audio_buffer.len());
            audio_buffer.drain(..).collect()
        };

        let (captured_sample_rate, captured_channels) = match self.captured_format.take() {
            Some(f) => f,
            None => {
                error!("Captured audio has no recorded format; discarding");
                return None;
            }
        };
        debug!("Captured format: {}Hz, {} channels", captured_sample_rate, captured_channels);
        debug!("Desired format: {}Hz, {} channels", desired_sample_rate, desired_channels);

        let mut processed_audio = audio_data;
        let initial_len = processed_audio.len();

        if captured_channels == 2 && desired_channels == 1 {
            processed_audio = stereo_to_mono(&processed_audio);
            debug!("Converted stereo to mono: {} -> {} samples", initial_len, processed_audio.len());
        } else if captured_channels > 2 {
            let samples_per_frame = captured_channels as usize;
            let mut mono_data = Vec::with_capacity(processed_audio.len() / samples_per_frame);
            for chunk in processed_audio.chunks_exact(samples_per_frame) {
                let average = chunk.iter().sum::<f32>() / samples_per_frame as f32;
                mono_data.push(average);
            }
            processed_audio = mono_data;
            debug!("Converted multi-channel to mono: {} -> {} samples", initial_len, processed_audio.len());
        }

        if captured_sample_rate != desired_sample_rate {
            let before_resample = processed_audio.len();
            processed_audio = audio_resample(
                &processed_audio,
                captured_sample_rate,
                desired_sample_rate,
                desired_channels,
            );
            debug!("Resampled audio: {} -> {} samples", before_resample, processed_audio.len());
        }

        if processed_audio.is_empty() {
            warn!("Processed audio is empty after conversion");
            None
        } else {
            debug!("Successfully processed {} samples", processed_audio.len());
            Some(processed_audio)
        }
    }
}

impl Default for AudioManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AudioManager {
    fn drop(&mut self) {
        self.stop_capture();
    }
}
