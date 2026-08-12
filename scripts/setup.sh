#!/bin/bash
# One-shot setup for Whispr on a fresh Mac (mirrors Batuhan's setup).
#
# Installs: Ollama + gemma4:e4b-mlx, whisper large-v3-turbo model,
#           whispr.app from the latest GitHub release, and writes the
#           same ~/.whispr/settings.json used on the dev machine.
#
# Usage:  bash scripts/setup.sh
set -euo pipefail

REPO="aycibatuhan/whispr"
WHISPER_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
CORRECTION_MODEL="gemma4:e4b-mlx"

echo "==> 1/5 Installing Ollama (if missing)"
if ! command -v ollama >/dev/null 2>&1; then
  if command -v brew >/dev/null 2>&1; then
    brew install ollama
  else
    curl -fsSL https://ollama.com/install.sh | sh
  fi
else
  echo "    Ollama already installed"
fi

echo "==> 2/5 Pulling correction model: $CORRECTION_MODEL"
ollama pull "$CORRECTION_MODEL"

echo "==> 3/5 Downloading Whisper model (large-v3-turbo, ~1.5GB)"
mkdir -p ~/.whispr
if [ ! -f ~/.whispr/model.bin ]; then
  curl -L -o ~/.whispr/model.bin "$WHISPER_URL"
else
  echo "    model.bin already present"
fi

echo "==> 4/5 Writing ~/.whispr/settings.json (same as dev machine)"
cat > ~/.whispr/settings.json <<'JSON'
{
  "audio": {
    "device_name": null,
    "remove_silence": true,
    "silence_threshold": 0.9,
    "min_silence_duration": 250,
    "recordings_dir": ".whispr"
  },
  "developer": {
    "save_recordings": false,
    "whisper_logging": false,
    "logging": true
  },
  "whisper": {
    "model_name": "large-v3-turbo",
    "language": null,
    "translate": false,
    "dictionary": null
  },
  "postprocess": {
    "enabled": true,
    "model": "gemma4:e4b-mlx",
    "system_prompt": "You are a post-processor for speech-to-text output. Fix transcription errors: spelling, spacing, misheard words, missing punctuation. Remove filler words (um, uh, yani, şey, işte, like, you know). Keep the original language and meaning (Turkish or English). Return only the corrected text with no explanations, no quotes, no preamble.",
    "timeout_secs": 30
  },
  "start_at_login": false,
  "keyboard_shortcut": "right_control_key",
  "model": {
    "display_name": "Whisper Large v3 Turbo",
    "url": "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
    "filename": "ggml-large-v3-turbo.bin"
  }
}
JSON

echo "==> 5/5 Downloading and installing whispr.app (latest release)"
if [ ! -d /Applications/whispr.app ]; then
  DMG_URL=$(curl -s "https://api.github.com/repos/$REPO/releases/latest" \
    | python3 -c "import json,sys; d=json.load(sys.stdin); print([a['browser_download_url'] for a in d['assets'] if a['name'].endswith('.dmg')][0])")
  curl -L -o /tmp/whispr.dmg "$DMG_URL"
  hdiutil attach /tmp/whispr.dmg -nobrowse -quiet
  cp -R "/Volumes/whispr/whispr.app" /Applications/
  hdiutil detach /Volumes/whispr -quiet
  rm /tmp/whispr.dmg
  echo "    Installed to /Applications/whispr.app"
else
  echo "    whispr.app already installed"
fi

echo ""
echo "=============================================="
echo "✅ Setup complete. Next steps:"
echo ""
echo "1. First launch (Gatekeeper bypass, one-time):"
echo "     xattr -dr com.apple.quarantine /Applications/whispr.app"
echo "     open /Applications/whispr.app"
echo ""
echo "2. Grant permissions when prompted (or via"
echo "   System Settings → Privacy & Security):"
echo "     - Microphone"
echo "     - Input Monitoring"
echo "     - Accessibility"
echo "   (Accessibility may need a manual toggle + app restart)"
echo ""
echo "3. Start Ollama (if not running):"
echo "     ollama serve &"
echo ""
echo "4. Use it: hold Right Control, speak, release."
echo "   Hotkey can be changed in the tray menu."
echo "=============================================="
