use crate::clients::DropboxClient;
use crate::models::RemotePath;
use crate::storage::Storage;
use anyhow::Result;

pub async fn generate_index(
    storage: &Storage,
    dropbox: &dyn DropboxClient,
    folder: &str,
) -> Result<()> {
    let files = storage.get_files_in_folder(folder).await?;
    if files.is_empty() {
        return Ok(());
    }

    let mut markdown = String::from("| Title | Authors | Summary |\n| :--- | :--- | :--- |\n");

    for file in files {
        let title = file.title.unwrap_or_else(|| "Unknown".to_string());
        let authors = file.authors.unwrap_or_else(|| "[]".to_string());
        let authors_list: Vec<String> = serde_json::from_str(&authors).unwrap_or_default();
        let summary = file.summary.unwrap_or_default();

        // Extract filename from target_path for relative link
        let filename = if let Some(path) = file.target_path {
            path.split('/').last().unwrap_or("").to_string()
        } else {
            "".to_string()
        };

        markdown.push_str(&format!(
            "| [{}]({}) | {} | {} |\n",
            title,
            filename,
            authors_list.join(", "),
            summary
        ));
    }

    let readme_path = RemotePath(format!("{}/README.md", folder));
    dropbox
        .upload_file(&readme_path, markdown.into_bytes())
        .await?;

    Ok(())
}
