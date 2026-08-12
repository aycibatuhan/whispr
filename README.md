<div align="center">
  <img src="src/icon.png" alt="Whispr Logo" width="128" height="128">
  <h1>Whispr</h1>
  <p><em>Your voice, your keyboard, no cloud required 🎙️</em></p>
</div>

Whispr is a macOS menubar application written in Rust for local voice-to-text transcription using [Whisper.cpp](https://github.com/ggerganov/whisper.cpp).

**Note:** Apple Silicon is required to run Whispr.

## Features

- Push-to-talk (right ⌘ Command key by default)
- Local processing
- Real-time transcription
- Menubar integration
- Configurable input and models
- Remove silence to prevent hallucination
- Custom vocabulary/dictionary based on config (to improve transcription quality with 'uncommon' words)

## Usage

1. The app requires a [Whisper.cpp](https://github.com/ggerganov/whisper.cpp) compatible model to be downloaded and placed in `~/.whispr/model.bin`
   - I highly recommend Whisper Large V3 Turbo
   - Download link: [ggml-large-v3-turbo.bin](https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin)
   - ```bash
     mkdir -p ~/.whispr && wget https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin -O ~/.whispr/model.bin
     ```
2. Launch Whispr
3. Hold right ⌘ Command
4. Speak
5. Release to insert text
6. Right click Whispr menubar to configure
   <div align="center">
     <img src="docs/assets/menubar.png" alt="Whispr Menubar Configuration" width="300">
   </div>

## Known Issues

- Startup experience is pretty rough, downloading the model and granting permissions.
- Silence removal is not tweaked yet and it is static, ideally it should be dynamic.
- Sometimes when right-clicking the menu bar icon, the menu doesn't open but flickers.
- Manually downloading the model is painful.
- The overlay lags when Whisper runs.

## ⚙️ Configuration

Whispr is highly configurable through its settings:

- **Audio Settings**
  - Choose input device
  - Silence removal
  - Recording options

- **Model Options**
  - Multiple Whisper models available
  - Language selection
  - Translation capabilities

- **Developer Features**
  - Save recordings for debugging
  - Enable Whisper logging
  - Detailed configuration options

- **Local LLM Post-Processing** (new)
  - Optional correction of transcriptions by a local LLM via [Ollama](https://ollama.com)
  - Removes filler words, fixes spelling/spacing/misheard words, adds punctuation
  - Fully local: nothing leaves your machine
  - Toggle in the tray menu: **Post-Processing → Enable Post-Processing**
  - Pick any installed Ollama model from the tray menu (list is loaded at app launch)
  - Falls back to raw transcription if Ollama is unreachable or times out
  - Post-processing verified with `gemma4:e4b-mlx` in 8 languages: English, Turkish, German, Spanish, French, Italian, Portuguese, Dutch

## Getting Started

1. Install [Ollama](https://ollama.com) and pull a correction model: `ollama pull gemma4:e4b-mlx`
2. Download a multilingual Whisper model to `~/.whispr/model.bin`:
   ```bash
   mkdir -p ~/.whispr && curl -L -o ~/.whispr/model.bin \
     https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
   ```
3. Launch Whispr
4. Right click the menubar icon → **Post-Processing → Enable Post-Processing**
5. Hold right ⌘ Command (or your configured hotkey) to speak
6. Release — the corrected text is inserted at your cursor

## Building from source

```bash
npm install
./scripts/build.sh          # auto-detects your Apple Development certificate
open src-tauri/target/debug/bundle/macos/whispr.app
```

`scripts/build.sh` signs with your Apple Development certificate if present (keeps
macOS permission grants across rebuilds) and falls back to adhoc signing otherwise.

<div align="center">
  <img src="docs/assets/menubar.png" alt="Whispr Menubar Configuration" width="300">
</div>

## Advanced usage

The advanced configuration for Whispr is located in `~/.whispr/settings.json`. Below is an example of the parameters you can configure:

```json
{
  "audio": {
    "device_name": "MacBook Pro Microphone",
    "remove_silence": true,
    "silence_threshold": 0.9,
    "min_silence_duration": 250,
    "recordings_dir": ".whispr"
  },
  "developer": {
    "save_recordings": true,
    "whisper_logging": false
  },
  "whisper": {
    "model_name": "large-v3-turbo",
    "language": "auto",
    "translate": false,
    "dictionary": ["USail", "CustomWord"]
  },
  "postprocess": {
    "enabled": true,
    "model": "gemma4:e4b-mlx",
    "system_prompt": "You are a post-processor for speech-to-text output. Fix transcription errors: spelling, spacing, misheard words, missing punctuation. Remove filler words (um, uh, like, you know). Keep the original language and meaning. Return only the corrected text with no explanations, no quotes, no preamble.",
    "timeout_secs": 30
  },
  "start_at_login": false,
  "keyboard_shortcut": "right_command_key",
  "model": {
    "display_name": "Whisper Large v3 Turbo",
    "url": "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
    "filename": "ggml-large-v3-turbo.bin"
  }
}
```

## Roadmap

- [ ] Model Management: Automated model downloads
- [ ] Headless experience & redesign status icon
  - The overlay is actually not needed at all, add a headless mode, use menubar icon coloring as recording indicator.
- [ ] Meeting mode with diarization and system audio recording
  - Memo: https://github.com/Mnpn/Azayaka/blob/main/Azayaka/Recording.swift, https://github.com/insidegui/AudioCap/blob/main/AudioCap/ProcessTap/CoreAudioUtils.swift
- [ ] Application context awareness
  - We can use a small local model, feed it a OCR'ed version of the current active window, the cursor position and much more in a customizable prompt template to postprocess the transcription, allowing more expressive interaction.
- [x] MLX-powered LLM post-processing
  - Implemented in this fork via Ollama (Metal-accelerated on Apple Silicon); see Post-Processing above
  - [ ] Apple Vision API integration
- [ ] Add Windows support
- [ ] Replacements
- [ ] GitHub Actions for Builds and Releases
- [ ] Automate builds/releases using GitHub Actions.
- [ ] Brew formulae

## Contributing

Open source project - contributions welcome.

## License

MIT License

---

<div align="center">
  <p>Made with ❤️ in Germany together with Claude</p>
</div>
