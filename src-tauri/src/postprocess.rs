use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PostProcessSettings {
    pub enabled: bool,
    pub model: String,
    pub system_prompt: String,
    pub timeout_secs: u64,
}

impl Default for PostProcessSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            model: "gemma4:e4b".to_string(),
            system_prompt: "You are a post-processor for speech-to-text output. \
Fix transcription errors: spelling, spacing, misheard words, missing punctuation. \
Remove filler words (um, uh, yani, şey, işte, like, you know). \
Keep the original language and meaning (Turkish or English). \
Return only the corrected text with no explanations, no quotes, no preamble."
                .to_string(),
            timeout_secs: 30,
        }
    }
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

pub fn correct(transcription: &str, settings: &PostProcessSettings) -> Result<String, String> {
    let body = serde_json::json!({
        "model": settings.model,
        "prompt": transcription,
        "system": settings.system_prompt,
        "stream": false,
        "think": false,
        "keep_alive": "10m",
        "options": { "temperature": 0.0 }
    });

    let resp = ureq::post("http://localhost:11434/api/generate")
        .timeout(Duration::from_secs(settings.timeout_secs))
        .send_json(body)
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("Ollama returned status {code}"),
            other => format!("Ollama request failed: {other}"),
        })?;

    let parsed: OllamaResponse = resp
        .into_json()
        .map_err(|e| format!("Bad Ollama response: {e}"))?;

    let corrected = parsed.response.trim().to_string();
    if corrected.is_empty() {
        return Err("Ollama returned empty response".to_string());
    }

    info!("Post-processed: {}", corrected);
    Ok(corrected)
}

pub fn list_models() -> Vec<String> {
    let resp = match ureq::get("http://localhost:11434/api/tags")
        .timeout(Duration::from_secs(3))
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            warn!("Could not reach Ollama: {e}");
            return Vec::new();
        }
    };

    #[derive(Deserialize)]
    struct OllamaTags {
        models: Vec<OllamaModel>,
    }
    #[derive(Deserialize)]
    struct OllamaModel {
        name: String,
    }

    match resp.into_json::<OllamaTags>() {
        Ok(tags) => tags.models.into_iter().map(|m| m.name).collect(),
        Err(e) => {
            warn!("Could not parse Ollama model list: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_cleans_transcription() {
        let settings = PostProcessSettings::default();
        let raw = "so um i was thinking we should like validate the seven tesla mri findings before we submit the paper okay";
        let corrected = correct(raw, &settings).expect("Ollama should respond");
        assert!(!corrected.is_empty());
        assert!(
            !corrected.contains("um") && !corrected.contains("like"),
            "fillers should be removed: {corrected}"
        );
        println!("RAW:       {raw}");
        println!("CORRECTED: {corrected}");
    }

    #[test]
    fn list_models_returns_ollama_models() {
        let models = list_models();
        assert!(!models.is_empty(), "Ollama should be running with models");
        println!("MODELS: {:?}", models);
    }
}
