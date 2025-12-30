use clap::{Parser, Subcommand};
use sci_librarian::setup_db;
use sci_librarian::storage::Storage;
use sci_librarian::pipeline::Pipeline;
use sci_librarian::indexing::generate_index;
use sci_librarian::clients::{HttpDropboxClient, HttpOpenRouterClient, DropboxClient, OpenRouterClient};
use anyhow::Result;
use std::sync::Arc;
use colored::*;
use std::env;

#[derive(Parser)]
#[command(name = "sci-librarian")]
#[command(about = "Organize scientific articles in Dropbox", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync, process, and index
    Run {
        #[arg(short, long, default_value_t = 4)]
        jobs: usize,
        #[arg(short, long, default_value_t = 10)]
        batch_size: i64,
    },
    /// Only sync new files from Dropbox
    Sync,
    /// Only process downloaded files
    Process {
        #[arg(short, long, default_value_t = 4)]
        jobs: usize,
    },
    /// Force regeneration of index for a path
    Index {
        #[arg(short, long)]
        path: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let db_url = "sqlite:state.db";
    let pool = setup_db(db_url).await?;
    let storage = Arc::new(Storage::new(pool));
    
    let dropbox_token = env::var("DROPBOX_TOKEN").expect("DROPBOX_TOKEN must be set");
    let openrouter_key = env::var("OPENROUTER_API_KEY").expect("OPENROUTER_API_KEY must be set");

    let dropbox: Arc<dyn DropboxClient> = Arc::new(HttpDropboxClient::new(dropbox_token));
    let openrouter: Arc<dyn OpenRouterClient> = Arc::new(HttpOpenRouterClient::new(openrouter_key));

    match cli.command {
        Commands::Run { jobs, batch_size } => {
            println!("{}", "Starting full run...".cyan().bold());
            
            // 1. Sync
            println!("Syncing from Dropbox...");
            let entries = dropbox.list_folder("/Inbox").await?;
            for entry in entries {
                storage.upsert_file(&entry.id, &entry.content_hash).await?;
            }

            // 2. Process
            let pipeline = Pipeline::new(storage.clone(), dropbox.clone(), openrouter.clone());
            pipeline.run_batch(batch_size, jobs).await?;
            
            println!("{}", "Run complete.".green());
        }
        Commands::Sync => {
            println!("Syncing from Dropbox...");
            let entries = dropbox.list_folder("/Inbox").await?;
            for entry in entries {
                storage.upsert_file(&entry.id, &entry.content_hash).await?;
            }
            println!("{}", "Sync complete.".green());
        }
        Commands::Process { jobs } => {
            println!("Processing pending files...");
            let pipeline = Pipeline::new(storage.clone(), dropbox.clone(), openrouter.clone());
            pipeline.run_batch(10, jobs).await?;
        }
        Commands::Index { path } => {
            println!("Indexing {}...", path);
            generate_index(&storage, &*dropbox, &path).await?;
            println!("{}", "Indexing complete.".green());
        }
    }

    Ok(())
}
