use lopdf::{Document, dictionary};
use sci_librarian::clients::{DropboxClient, DropboxEntry, FakeDropboxClient, FakeMistralClient};
use sci_librarian::models::Rules;
use sci_librarian::models::{
    ArticleMetadata, DropboxId, FileHash, OneLineSummary, RemotePath, Rule, WorkDirectory,
};
use sci_librarian::pipeline::Pipeline;
use sci_librarian::setup_db;
use sci_librarian::storage::Storage;

use std::fs;
use std::sync::Arc;

fn create_pdf(content: &str) -> Document {
    let mut doc = lopdf::Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! {
            "F1" => font_id,
        },
    });
    let content_bytes: &[u8] = content.as_bytes();
    let content_id = doc.add_object(lopdf::Stream::new(dictionary! {}, content_bytes.to_vec()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => vec![page_id.into()],
        "Count" => 1,
    };
    doc.objects
        .insert(pages_id, lopdf::Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc
}
#[tokio::test]
async fn test_full_scenario() {
    // 1. Setup
    let temp_dir = tempfile::tempdir().unwrap();
    let work_dir = WorkDirectory(temp_dir.path().to_path_buf());
    fs::create_dir_all(work_dir.0.join("raw")).unwrap();

    let db_path = work_dir.0.join("state.db");
    let db_url = format!("sqlite:///{}", db_path.to_string_lossy().replace('\\', "/"));
    let pool = setup_db(&db_url).await.unwrap();
    let storage = Arc::new(Storage::new(pool));
    let mut dropbox = FakeDropboxClient::new();
    let llm = FakeMistralClient::new();

    // Create a valid PDF using lopdf
    let mut doc = create_pdf("BT /F1 12 Tf 100 700 Td (Quantum Computing) Tj ET");

    let mut paper_content = Vec::new();
    doc.save_to(&mut paper_content).unwrap();

    let paper_id = DropboxId("id:123".to_string());
    let paper_path = RemotePath("/0_inbox/paper.pdf".to_string());
    let paper_hash = FileHash("hash123".to_string());

    dropbox
        .add_entry(
            DropboxEntry {
                id: paper_id.clone(),
                name: "paper.pdf".to_string(),
                path: paper_path.clone(),
                content_hash: paper_hash.clone(),
            },
            paper_content.clone(),
        )
        .await;

    let meta = ArticleMetadata {
        title: "Quantum Computing for Dummies".to_string(),
        authors: vec!["John Doe".to_string()],
        summary: OneLineSummary("A beginner's guide to quantum computing.".to_string()),
        abstract_text: "This paper explains quantum computing in simple terms.".to_string(),
    };
    let matching_rules = vec![Rule {
        name: String::from("Quantum Computing"),
        description: String::from("Everything about Quantum Computing"),
        path: RemotePath::from("/Research/Quantum_Computing"),
    }];
    llm.set_response("Quantum", meta.clone(), matching_rules.clone())
        .await;

    let dropbox = Arc::new(dropbox);
    let llm = Arc::new(llm);
    let rules = Arc::new(Rules::from(vec![
        Rule {
            name: String::from("AI"),
            description: String::from(
                "Neural Networks, Deep Learning, Large Language Models (LLMs), Reinforcement Learning and other large-scale text, image and video processing tasks using function approximators",
            ),
            path: RemotePath::from("/out/ai"),
        },
        Rule {
            name: String::from("Programming Languages"),
            description: String::from(
                "Programming language theory, parsers, compilers, partial evaluation, type systems etc.",
            ),
            path: RemotePath::from("/out/programming-languages"),
        },
    ]));
    let pipeline = Pipeline::new(
        storage.clone(),
        dropbox.clone(),
        llm.clone(),
        work_dir.clone(),
        rules,
    );

    // 2. Sync
    let entries = dropbox.list_folder("/0_inbox").await.unwrap();
    for entry in entries {
        storage
            .upsert_file(&entry.id, &entry.name, &entry.content_hash)
            .await
            .unwrap();
    }

    // Verify file name is stored
    let pending = storage.get_pending_files(10).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].file_name.as_deref(), Some("paper.pdf"));

    // 3. Run Pipeline
    pipeline.run_batch(10, 1).await.unwrap();

    // Final Verification
    let files = dropbox.files.lock().await;
    assert!(files.contains_key("/Research/Quantum_Computing/paper.pdf"));
    assert!(files.contains_key("/Research/Quantum_Computing/paper.pdf.md"));

    let sidecar = String::from_utf8(
        files
            .get("/Research/Quantum_Computing/paper.pdf.md")
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert!(sidecar.contains("# Quantum Computing for Dummies"));
    assert!(sidecar.contains("## Authors\nJohn Doe"));
    assert!(sidecar.contains("## Summary\nA beginner's guide to quantum computing."));
    assert!(
        sidecar.contains("## Abstract\nThis paper explains quantum computing in simple terms.")
    );
}
