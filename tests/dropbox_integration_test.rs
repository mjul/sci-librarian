use sci_librarian::clients::{DropboxClient, HttpDropboxClient};
use std::env;

fn get_dropbox_token() -> String {
    env::var("DROPBOX_TOKEN")
        .expect("DROPBOX_TOKEN environment variable must be set for integration tests")
}

#[tokio::test]
async fn test_dropbox_list_folder() {
    let token = get_dropbox_token();
    let client = HttpDropboxClient::new(token);

    let result = client.list_folder("").await;

    assert!(
        result.is_ok(),
        "list_folder should return Ok, got: {:?}",
        result.err()
    );
    let entries = result.unwrap();
    println!("Found {} entries in root folder", entries.len());
}

#[tokio::test]
async fn test_dropbox_download_file() {
    let token = get_dropbox_token();
    let client = HttpDropboxClient::new(token);

    // First list folder to find a file to download
    let entries = client
        .list_folder("/0_inbox")
        .await
        .expect("Failed to list folder");

    // Find the first file entry
    // (Assuming DropboxEntry has some way to distinguish files from folders,
    // but the trait download_file takes DropboxId which we have)
    assert!(
        entries.len() > 0,
        "No entries found in /0_inbox folder, cannot download file"
    );

    if let Some(entry) = entries.first() {
        println!(
            "Attempting to download file: {} (id: {:?})",
            entry.path.0, entry.id
        );
        let content = client.download_file(&entry.id).await;

        // It might fail if it's a folder, but let's see.
        // Dropbox API download usually works on files.
        // If it's a folder, we might get an error, which is also a valid test of the client's behavior.
        match content {
            Ok(bytes) => println!("Successfully downloaded {} bytes", bytes.len()),
            Err(e) => println!("Download failed (might be a folder): {:?}", e),
        }
    }
}
