use clap::{Parser, Subcommand};
use sci_librarian::setup_db;
use sci_librarian::storage::Storage;
use sci_librarian::pipeline::Pipeline;
use sci_librarian::indexing::generate_index;
use sci_librarian::clients::{HttpDropboxClient, HttpOpenRouterClient, DropboxClient, OpenRouterClient};
use sci_librarian::models::{WorkDirectory, DropboxInbox};
use anyhow::Result;
use std::sync::Arc;
use colored::*;
use std::env;
use std::path::PathBuf;
use std::fs;

#[derive(Parser)]
#[command(name = "sci-librarian")]
#[command(about = "Organize scientific articles in Dropbox", long_about = None)]
struct Cli {
    #[arg(short, long, global = true, default_value = "working")]
    work_directory: PathBuf,

    #[arg(short, long, global = true, default_value = "/0_inbox")]
    inbox: String,

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
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    // Initialize work directory
    fs::create_dir_all(&cli.work_directory)?;
    let work_dir_abs = if cli.work_directory.is_absolute() {
        cli.work_directory.clone()
    } else {
        env::current_dir()?.join(&cli.work_directory)
    };
    let work_dir = WorkDirectory(work_dir_abs.clone());
    println!("{}: {}", "Using working directory".cyan().bold(), work_dir_abs.to_string_lossy());

    let inbox = DropboxInbox(cli.inbox.clone());
    println!("{}: {}", "Using Dropbox inbox".cyan().bold(), inbox.0);

    // Ensure raw directory exists
    fs::create_dir_all(work_dir.0.join("raw"))?;

    let db_path = work_dir.0.join("state.db");
    let db_url = format!("sqlite:///{}", db_path.to_string_lossy().replace('\\', "/"));
    println!("{}: {}", "Using SQLite file".cyan().bold(), db_path.to_string_lossy());
    let pool = setup_db(&db_url).await?;
    let storage = Arc::new(Storage::new(pool));
    
    let dropbox_token = get_env_var("DROPBOX_TOKEN")?;
    let openrouter_key = get_env_var("OPENROUTER_API_KEY")?;

    let dropbox: Arc<dyn DropboxClient> = Arc::new(HttpDropboxClient::new(dropbox_token));
    let openrouter: Arc<dyn OpenRouterClient> = Arc::new(HttpOpenRouterClient::new(openrouter_key));

    match cli.command {
        Commands::Run { jobs, batch_size } => {
            println!("{}", "Starting full run...".cyan().bold());
            
            // 1. Sync
            println!("Syncing from Dropbox...");
            let entries = dropbox.list_folder(&inbox.0).await?;
            for entry in entries {
                storage.upsert_file(&entry.id, &entry.content_hash).await?;
            }

            // 2. Process
            let pipeline = Pipeline::new(storage.clone(), dropbox.clone(), openrouter.clone(), work_dir.clone());
            pipeline.run_batch(batch_size, jobs).await?;
            
            println!("{}", "Run complete.".green());
        }
        Commands::Sync => {
            println!("Syncing from Dropbox...");
            let entries = dropbox.list_folder(&inbox.0).await?;
            for entry in entries {
                storage.upsert_file(&entry.id, &entry.content_hash).await?;
            }
            println!("{}", "Sync complete.".green());
        }
        Commands::Process { jobs } => {
            println!("Processing pending files...");
            let pipeline = Pipeline::new(storage.clone(), dropbox.clone(), openrouter.clone(), work_dir.clone());
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

fn get_env_var(name: &str) -> Result<String> {
    env::var(name).map_err(|_| {
        anyhow::anyhow!(
            "Environment variable {} is not set.\n\n\
            {}:\n  $env:{} = \"your-token-here\"\n\n\
            {}:\n  export {}=\"your-token-here\"",
            name.bold().red(),
            "PowerShell".cyan().bold(),
            name,
            "Bash/Zsh".cyan().bold(),
            name
        )
    })
}
