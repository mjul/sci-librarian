use crate::models::{ArticleMetadata, DropboxId, FileHash, OneLineSummary, RemotePath};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

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
pub trait OpenRouterClient: Send + Sync {
    async fn query_llm(
        &self,
        text: &str,
        rules: &str,
    ) -> Result<(ArticleMetadata, Vec<RemotePath>)>;
}

pub struct HttpDropboxClient {
    token: String,
    client: reqwest::Client,
}

impl HttpDropboxClient {
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl DropboxClient for HttpDropboxClient {
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

        let res_raw = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Failed to send request to {}", url))?;

        let status = res_raw.status().clone();
        let res = res_raw.json::<serde_json::Value>().await.with_context(|| {
            format!(
                "HTTP request failed with status code {}, url: {}",
                status, url
            )
        })?;

        let mut entries = Vec::new();
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
        Ok(entries)
    }

    async fn download_file(&self, id: &DropboxId) -> Result<Vec<u8>> {
        let url = "https://content.dropboxapi.com/2/files/download";
        let arg = serde_json::json!({ "path": id.0 }).to_string();

        let res = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .header("Dropbox-API-Arg", arg)
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Download failed: {}", res.status()));
        }

        Ok(res.bytes().await?.to_vec())
    }

    async fn upload_file(&self, path: &RemotePath, content: Vec<u8>) -> Result<()> {
        let url = "https://content.dropboxapi.com/2/files/upload";
        let arg = serde_json::json!({
            "path": path.0,
            "mode": "overwrite",
            "autorename": true,
            "mute": false,
            "strict_conflict": false
        })
        .to_string();

        let res = self
            .client
            .post(url)
            .bearer_auth(&self.token)
            .header("Dropbox-API-Arg", arg)
            .header("Content-Type", "application/octet-stream")
            .body(content)
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(anyhow::anyhow!("Upload failed: {}", res.status()));
        }

        Ok(())
    }
}

pub struct HttpOpenRouterClient {
    api_key: String,
    client: reqwest::Client,
}

impl HttpOpenRouterClient {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl OpenRouterClient for HttpOpenRouterClient {
    async fn query_llm(
        &self,
        text: &str,
        rules: &str,
    ) -> Result<(ArticleMetadata, Vec<RemotePath>)> {
        let url = "https://openrouter.ai/api/v1/chat/completions";

        let prompt = format!(
            "Extract Title, Authors, Abstract from the following scientific paper text. \
            Provide a 1-line summary. \
            Match the abstract against these rules to select target paths: {}\n\n\
            Text:\n{}\n\n\
            Respond ONLY with JSON in this format: \
            {{\"title\": \"...\", \"authors\": [\"...\"], \"summary\": \"...\", \"abstract\": \"...\", \"targets\": [\"/Path/To/Folder/filename.pdf\"]}}",
            rules, text
        );

        let body = serde_json::json!({
            "model": "google/gemini-flash-1.5",
            "messages": [
                { "role": "user", "content": prompt }
            ],
            "response_format": { "type": "json_object" }
        });

        let res = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let content = res["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid LLM response"))?;

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

pub struct FakeOpenRouterClient {
    pub responses: Arc<Mutex<HashMap<String, (ArticleMetadata, Vec<RemotePath>)>>>,
}

impl FakeOpenRouterClient {
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
impl OpenRouterClient for FakeOpenRouterClient {
    async fn query_llm(
        &self,
        text: &str,
        _rules: &str,
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
