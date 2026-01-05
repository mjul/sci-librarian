use crate::models::{ArticleMetadata, DropboxId, FileHash, OneLineSummary, RemotePath, Rules};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct DropboxEntry {
    pub id: DropboxId,
    pub path: RemotePath,
    pub content_hash: FileHash,
}

#[async_trait]
pub trait DropboxClient: Send + Sync {
    async fn list_folder(&self, path: &str) -> Result<Vec<DropboxEntry>>;
    async fn download_file(&self, id: &DropboxId) -> Result<Vec<u8>>;
    async fn upload_file(&self, path: &RemotePath, content: Vec<u8>) -> Result<()>;
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn query_llm(
        &self,
        text: &str,
        rules: &Rules,
    ) -> Result<(ArticleMetadata, Vec<RemotePath>)>;
}

pub struct DropboxHttpClient {
    token: String,
    client: reqwest::Client,
}

/** Time-out for HTTP requests to the Dropbox API */
const DROPBOX_HTTP_TIMEOUT_IN_SECONDS: u64 = 3;

impl DropboxHttpClient {
    pub fn new(token: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                DROPBOX_HTTP_TIMEOUT_IN_SECONDS,
            ))
            .build()
            .unwrap();
        Self { token, client }
    }

    /// Send a POST request to Dropbox API.
    async fn dropbox_post_request(
        &self,
        url: &str,
        body: Option<Vec<u8>>,
        api_arg: Option<&str>,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response> {
        debug!("Sending POST request to Dropbox API: {}", url);
        let mut request = self.client.post(url).bearer_auth(&self.token);

        if let Some(arg) = api_arg {
            request = request.header("Dropbox-API-Arg", arg);
        }

        if let Some(ct) = content_type {
            request = request.header("Content-Type", ct);
        }

        if let Some(b) = body {
            request = request.body(b);
        }

        let res_raw = request
            .send()
            .await
            .with_context(|| format!("Failed to send request to {}", url))?;

        let status = res_raw.status();
        if !status.is_success() {
            let error_text = res_raw.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Dropbox API error ({}): {}",
                status,
                error_text
            ));
        }

        Ok(res_raw)
    }

    fn append_entries(&self, entries: &mut Vec<DropboxEntry>, res: &serde_json::Value) {
        if let Some(list) = res["entries"].as_array() {
            for item in list {
                if item[".tag"] == "file" {
                    entries.push(DropboxEntry {
                        id: DropboxId(item["id"].as_str().unwrap_or_default().to_string()),
                        path: RemotePath(
                            item["path_display"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        ),
                        content_hash: FileHash(
                            item["content_hash"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                        ),
                    });
                }
            }
        }
    }
}

const DROPBOX_ALLOWED_UPLOAD_PREFIX: &'static str = "/dev-sci-librarian/";

#[async_trait]
impl DropboxClient for DropboxHttpClient {
    async fn list_folder(&self, path: &str) -> Result<Vec<DropboxEntry>> {
        let url = "https://api.dropboxapi.com/2/files/list_folder";
        let body = serde_json::json!({
            "path": path,
            "recursive": false,
            "include_media_info": false,
            "include_deleted": false,
            "include_has_explicit_shared_members": false,
            "include_mounted_folders": true,
            "include_non_downloadable_files": true
        });

        let body_bytes = serde_json::to_vec(&body)?;
        let res_raw = self
            .dropbox_post_request(url, Some(body_bytes), None, Some("application/json"))
            .await
            .with_context(|| format!("Failed to list folder at {}", path))?;

        let res: serde_json::Value = res_raw
            .json()
            .await
            .with_context(|| format!("Failed to parse JSON response from {}", url))?;

        let mut all_entries = Vec::new();
        self.append_entries(&mut all_entries, &res);

        let mut current_res = res;
        while current_res["has_more"].as_bool().unwrap_or(false) {
            let cursor = current_res["cursor"].as_str().ok_or_else(|| {
                anyhow::anyhow!("Missing cursor in Dropbox response despite has_more=true")
            })?;

            let continue_url = "https://api.dropboxapi.com/2/files/list_folder/continue";
            let continue_body = serde_json::json!({ "cursor": cursor });
            let continue_body_bytes = serde_json::to_vec(&continue_body)?;

            let res_raw = self
                .dropbox_post_request(
                    continue_url,
                    Some(continue_body_bytes),
                    None,
                    Some("application/json"),
                )
                .await
                .with_context(|| format!("Failed to list folder continuation at {}", path))?;

            current_res = res_raw
                .json()
                .await
                .with_context(|| format!("Failed to parse JSON response from {}", continue_url))?;

            self.append_entries(&mut all_entries, &current_res);
        }

        Ok(all_entries)
    }

    async fn download_file(&self, id: &DropboxId) -> Result<Vec<u8>> {
        let url = "https://content.dropboxapi.com/2/files/download";
        let arg = serde_json::json!({ "path": id.0 }).to_string();

        let res_raw = self
            .dropbox_post_request(url, None, Some(&arg), None)
            .await
            .with_context(|| format!("Failed to download file {}", id.0))?;

        Ok(res_raw.bytes().await?.to_vec())
    }

    async fn upload_file(&self, path: &RemotePath, content: Vec<u8>) -> Result<()> {
        // Check allowed paths, for extra safety (hard-coded for now)
        if !path.0.starts_with(DROPBOX_ALLOWED_UPLOAD_PREFIX) {
            return Err(anyhow::anyhow!(format!(
                "Upload path not allowed to path: {} (allowed prefix: {})",
                path.0, DROPBOX_ALLOWED_UPLOAD_PREFIX
            )));
        }

        let url = "https://content.dropboxapi.com/2/files/upload";
        let arg = serde_json::json!({
            "path": path.0,
            "mode": "overwrite",
            "autorename": true,
            "mute": false,
            "strict_conflict": false
        })
        .to_string();

        self.dropbox_post_request(
            url,
            Some(content),
            Some(&arg),
            Some("application/octet-stream"),
        )
        .await
        .with_context(|| format!("Failed to upload file to {}", path.0))?;

        Ok(())
    }
}

pub struct MistralHttpClient {
    api_key: String,
    client: reqwest::Client,
}

impl MistralHttpClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl LlmClient for MistralHttpClient {
    async fn query_llm(
        &self,
        text: &str,
        rules: &Rules,
    ) -> Result<(ArticleMetadata, Vec<RemotePath>)> {
        let url = "https://api.mistral.ai/v1/chat/completions";

        // Transform the rules to a String:
        let rules_str = rules.0.iter().map(|rule| format!("Description: {} -> <target>{}</target>", rule.description, rule.target)).collect::<Vec<String>>().join("\n");

        let prompt = format!(
            "Extract Title, Authors, Abstract from the following scientific paper text. \
            Provide a 1-line summary. \
            Match the abstract against these rules to select target paths: \n\n\
            <rules>\n\
            {}\
            </rules>\n\n\
            Text:\n\n\
            <text>\
            {}\
            </text>\n\n\
            Respond ONLY with JSON in this format, where targets are from any matching rules: \
            {{\"title\": \"...\", \"authors\": [\"...\"], \"summary\": \"...\", \"abstract\": \"...\", \"targets\": [\"...\",\"...\"]}}",
            rules_str, text
        );

        let body = serde_json::json!({
            "model": "mistral-small-latest",
            "messages": [
                { "role": "user", "content": prompt }
            ],
            "response_format": { "type": "json_object" }
        });

        debug!("Mistral prompt: {}", prompt);

        let res = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Failed to send request to {}", url))?
            .json::<serde_json::Value>()
            .await?;

        let content = res["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid LLM response"))?;

        debug!("Mistral response content: {}", content);

        let parsed: serde_json::Value = serde_json::from_str(content)?;

        let meta = ArticleMetadata {
            title: parsed["title"].as_str().unwrap_or("Unknown").to_string(),
            authors: parsed["authors"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default(),
            summary: OneLineSummary(parsed["summary"].as_str().unwrap_or_default().to_string()),
            abstract_text: parsed["abstract"].as_str().unwrap_or_default().to_string(),
        };

        let targets = parsed["targets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| RemotePath(v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Ok((meta, targets))
    }
}

pub struct FakeDropboxClient {
    pub files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pub entries: Vec<DropboxEntry>,
}

impl FakeDropboxClient {
    pub fn new() -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            entries: Vec::new(),
        }
    }

    pub async fn add_entry(&mut self, entry: DropboxEntry, content: Vec<u8>) {
        self.entries.push(entry.clone());
        let mut files = self.files.lock().await;
        files.insert(entry.id.0.clone(), content);
    }
}

#[async_trait]
impl DropboxClient for FakeDropboxClient {
    async fn list_folder(&self, _path: &str) -> Result<Vec<DropboxEntry>> {
        Ok(self.entries.clone())
    }

    async fn download_file(&self, id: &DropboxId) -> Result<Vec<u8>> {
        let files = self.files.lock().await;
        files
            .get(&id.0)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("File not found"))
    }

    async fn upload_file(&self, path: &RemotePath, content: Vec<u8>) -> Result<()> {
        let mut files = self.files.lock().await;
        files.insert(path.0.clone(), content);
        Ok(())
    }
}

pub struct FakeMistralClient {
    pub responses: Arc<Mutex<HashMap<String, (ArticleMetadata, Vec<RemotePath>)>>>,
}

impl FakeMistralClient {
    pub fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn set_response(
        &self,
        text_snippet: &str,
        meta: ArticleMetadata,
        targets: Vec<RemotePath>,
    ) {
        let mut responses = self.responses.lock().await;
        responses.insert(text_snippet.to_string(), (meta, targets));
    }
}

#[async_trait]
impl LlmClient for FakeMistralClient {
    async fn query_llm(
        &self,
        text: &str,
        _rules: &Rules,
    ) -> Result<(ArticleMetadata, Vec<RemotePath>)> {
        let responses = self.responses.lock().await;
        for (snippet, response) in responses.iter() {
            if text.contains(snippet) {
                return Ok(response.clone());
            }
        }

        // Default response if no snippet matches
        Ok((
            ArticleMetadata {
                title: "Unknown Paper".to_string(),
                authors: vec!["Unknown Author".to_string()],
                summary: OneLineSummary("A paper about something.".to_string()),
                abstract_text: "This is a default abstract.".to_string(),
            },
            vec![RemotePath("/Archive/General".to_string())],
        ))
    }
}
