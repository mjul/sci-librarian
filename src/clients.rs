use crate::models::{
    ArticleMetadata, DropboxId, FileHash, OneLineSummary, RemotePath, Rule, Rules,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct DropboxEntry {
    pub id: DropboxId,
    pub name: String,
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
    /// Query the LLM for metadata and any matching rules for the given text.
    async fn query_llm(&self, text: &str, rules: &Rules) -> Result<(ArticleMetadata, Vec<Rule>)>;
}

pub struct DropboxHttpClient {
    token: String,
    client: reqwest::Client,
    allowed_upload_prefix: String,
}

/** Time-out for HTTP requests to the Dropbox API */
const DROPBOX_HTTP_TIMEOUT_IN_SECONDS: u64 = 3;

impl DropboxHttpClient {
    /// Create a Dropbox client with an API token and allowed upload prefix as a safe-guard against
    /// uploading files outside the allowed directory.
    pub fn new(token: String, allowed_upload_prefix: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                DROPBOX_HTTP_TIMEOUT_IN_SECONDS,
            ))
            .build()
            .unwrap();
        Self {
            token,
            client,
            allowed_upload_prefix,
        }
    }

    /// Send a POST request to Dropbox API.
    async fn dropbox_post_request(
        &self,
        url: &str,
        body: Option<Vec<u8>>,
        api_arg: Option<&str>,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response> {
        tracing::debug!("Sending POST request to Dropbox API: {}", url);
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
                        name: item["name"].as_str().unwrap_or_default().to_string(),
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
        // Check allowed paths, for extra safety
        if !path.0.starts_with(&self.allowed_upload_prefix) {
            return Err(anyhow::anyhow!(format!(
                "Upload path not allowed to path: {} (allowed prefix: {})",
                path.0, &self.allowed_upload_prefix
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
    async fn query_llm(&self, text: &str, rules: &Rules) -> Result<(ArticleMetadata, Vec<Rule>)> {
        let url = "https://api.mistral.ai/v1/chat/completions";

        // Transform the rules to a String:
        let rules_str = rules
            .0
            .iter()
            .map(|rule| {
                format!(
                    "Category: <name>{}</name> <description>{}</description>",
                    rule.name, rule.description
                )
            })
            .collect::<Vec<String>>()
            .join("\n");

        let prompt = format!(
            "Extract Title, Authors, Abstract from the following scientific paper text. \
            Provide a 1-line summary. \
            Match the abstract against these categories to select the applicable categories for the \
            text.  \n\n\
            <categories>\n\
            {}\
            </categories>\n\n\
            Text:\n\n\
            <text>\
            {}\
            </text>\n\n\
            Respond ONLY with JSON in this format, where the \"categories\" key has an array of \
            strings with the exact names of the categories matched to the text:  \n\n\
            {{\"title\": \"...\", \"authors\": [\"...\"], \"summary\": \"...\", \"abstract\": \"...\", \"categories\": [\"...\",\"...\"]}}",
            rules_str, text
        );

        let body = serde_json::json!({
            "model": "mistral-small-latest",
            "messages": [
                { "role": "user", "content": prompt }
            ],
            "response_format": { "type": "json_object" }
        });

        tracing::debug!("Mistral prompt: {}", prompt);

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

        tracing::debug!("Mistral response content: {}", content);

        let parsed: serde_json::Value = serde_json::from_str(content)?;

        // Verify that the response has the expected keys:
        let title = parsed["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("LLM response has no title"))?;
        let author_values = parsed["authors"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("LLM response has no authors"))?;
        let summary = parsed["summary"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("LLM response has no summary"))?;
        let abstract_text = parsed["abstract"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("LLM response has no abstract"))?;
        let category_values = parsed["categories"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("LLM response has no categories"))?;

        // Verify that all authors are strings
        if !author_values.iter().all(|c| c.is_string()) {
            return Err(anyhow::anyhow!("LLM response has non-string authors"));
        }
        let authors = author_values
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect::<Vec<String>>();

        // Verify that all categories are strings
        if !category_values.iter().all(|c| c.is_string()) {
            return Err(anyhow::anyhow!("LLM response has non-string categories"));
        }
        let categories = category_values
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect::<Vec<String>>();

        let meta = ArticleMetadata {
            title: String::from(title),
            authors: authors,
            summary: OneLineSummary(String::from(summary)),
            abstract_text: String::from(abstract_text),
        };

        let unique_matching_rule_names = categories.iter().collect::<HashSet<_>>();
        let rules_by_name = rules
            .0
            .iter()
            .map(|rule: &Rule| (rule.name.clone(), rule))
            .collect::<HashMap<String, &Rule>>();
        let (known_matches_rule_names, unknown_matched_rule_names): (Vec<_>, Vec<_>) =
            unique_matching_rule_names
                .into_iter()
                .partition(|name| rules_by_name.contains_key(*name));
        if !unknown_matched_rule_names.is_empty() {
            tracing::warn!(
                "LLM response included unknown rule names: {:?}",
                unknown_matched_rule_names
            );
        }
        tracing::debug!(
            "LLM response matched rules: {:?}",
            &known_matches_rule_names
        );
        let matching_rules: Vec<Rule> = known_matches_rule_names
            .into_iter()
            .filter_map(|name| rules_by_name.get(name).map(|rule| (*rule).clone()))
            .collect();

        tracing::debug!("Extracted metadata: {:#?}", meta);
        tracing::debug!("Found matching rules: {:#?}", matching_rules);

        Ok((meta, matching_rules))
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
    pub responses: Arc<Mutex<HashMap<String, (ArticleMetadata, Vec<Rule>)>>>,
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
        matching_rules: Vec<Rule>,
    ) {
        let mut responses = self.responses.lock().await;
        responses.insert(text_snippet.to_string(), (meta, matching_rules));
    }
}

#[async_trait]
impl LlmClient for FakeMistralClient {
    async fn query_llm(&self, text: &str, _rules: &Rules) -> Result<(ArticleMetadata, Vec<Rule>)> {
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
            vec![],
        ))
    }
}
