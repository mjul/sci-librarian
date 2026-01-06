use anyhow::{Error, Result};
use clap::{Parser, Subcommand};
use colored::*;
use sci_librarian::clients::{DropboxClient, DropboxHttpClient, LlmClient, MistralHttpClient};
use sci_librarian::indexing::generate_index;
use sci_librarian::models::{DropboxInbox, Rule, Rules, WorkDirectory};
use sci_librarian::pipeline::Pipeline;
use sci_librarian::setup_db;
use sci_librarian::storage::Storage;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, debug};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[derive(Parser)]
#[command(name = "sci-librarian")]
#[command(about = "Organize scientific articles in Dropbox", long_about = None)]
struct Cli {
    /// Path to the application working directory with state database and temporary files.
    #[arg(short, long, global = true, default_value = "working")]
    work_directory: PathBuf,

    /// Path to application inbox. This is where files are picked up for processing.
    #[arg(
        short,
        long,
        global = true,
        default_value = "",
        long_help = "If your app is restricted to just its own folder under Apps, the path to that folder is the empty string. If you bravely gave it access to your whole Dropbox account, the root folder is the empty string, all other folders start with a '/'."
    )]
    inbox: String,

    #[command(subcommand)]
    command: Commands,
}

const DEFAULT_JOBS: usize = 4;
const DEFAULT_BATCH_SIZE: i64 = 10;

#[derive(Subcommand)]
enum Commands {
    /// Sync, process, and index
    Run {
        #[arg(short, long, default_value_t = DEFAULT_JOBS)]
        jobs: usize,
        #[arg(short, long, default_value_t = DEFAULT_BATCH_SIZE)]
        batch_size: i64,
    },
    /// Only sync new files from Dropbox
    Sync,
    /// Only process downloaded files
    Process {
        #[arg(short, long, default_value_t = DEFAULT_JOBS)]
        jobs: usize,
        #[arg(short, long, default_value_t = DEFAULT_BATCH_SIZE)]
        batch_size: i64,
    },
    /// Force regeneration of index for a path
    Index {
        #[arg(short, long)]
        path: String,
    },
}

// TODO: Get this as a parameter
const DROPBOX_ALLOWED_UPLOAD_PREFIX: &'static str = "/sorted";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_default_env())
        .init();

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
    info!(
        "{}: {}",
        "Using working directory".cyan().bold(),
        work_dir_abs.to_string_lossy()
    );

    let inbox = DropboxInbox(cli.inbox.clone());
    info!("{}: {}", "Using Dropbox inbox".cyan().bold(), inbox.0);

    // Ensure raw directory exists
    fs::create_dir_all(work_dir.0.join("raw"))?;

    let db_path = work_dir.0.join("state.db");
    let db_url = format!("sqlite:///{}", db_path.to_string_lossy().replace('\\', "/"));
    info!(
        "{}: {}",
        "Using SQLite file".cyan().bold(),
        db_path.to_string_lossy()
    );
    let pool = setup_db(&db_url).await?;
    let storage = Arc::new(Storage::new(pool));

    let dropbox_token = get_env_var("DROPBOX_TOKEN")?;
    let mistral_key = get_env_var("MISTRAL_API_KEY")?;


    let dropbox: Arc<dyn DropboxClient> = Arc::new(DropboxHttpClient::new(dropbox_token, String::from(DROPBOX_ALLOWED_UPLOAD_PREFIX)));
    let llm: Arc<dyn LlmClient> = Arc::new(MistralHttpClient::new(mistral_key));

    let rules = Arc::new(get_rules());

    match cli.command {
        Commands::Run { jobs, batch_size } => {
            info!("{}", "Starting full run...".cyan().bold());
            execute_sync(&inbox, &storage, &dropbox).await?;
            execute_process(rules, work_dir, &storage, &dropbox, llm, jobs, batch_size).await?;
            info!("{}", "Run complete.".green());
        }
        Commands::Sync => {
            execute_sync(&inbox, &storage, &dropbox).await?;
        }
        Commands::Process { jobs, batch_size } => {
            execute_process(rules, work_dir, &storage, &dropbox, llm, jobs, batch_size).await?;
        }
        Commands::Index { path } => {
            execute_index(&storage, dropbox, &path).await?;
        }
    }

    Ok(())
}

fn get_rules() -> Rules {
    Rules::from(vec![
        Rule {
            description: String::from(
                "Neural Networks, Deep Learning, Large Language Models (LLMs), Reinforcement Learning and other large-scale text, image and video processing tasks using function approximators",
            ),
            target: String::from("/sorted/ai"),
        },
        Rule {
            description: String::from(
                "Programming language theory, parsers, compilers, partial evaluation, type systems etc.",
            ),
            target: String::from("/sorted/databases"),
        },
    ])
}

async fn execute_index(
    storage: &Arc<Storage>,
    dropbox: Arc<dyn DropboxClient>,
    path: &String,
) -> Result<(), Error> {
    println!("Indexing {}...", path);
    generate_index(&storage, &*dropbox, &path).await?;
    println!("{}", "Indexing complete.".green());
    Ok(())
}

async fn execute_process(
    rules: Arc<Rules>,
    work_dir: WorkDirectory,
    storage: &Arc<Storage>,
    dropbox: &Arc<dyn DropboxClient>,
    llm: Arc<dyn LlmClient>,
    jobs: usize,
    batch_size: i64,
) -> Result<(), Error> {
    println!("Processing pending files...");
    let pipeline = Pipeline::new(
        storage.clone(),
        dropbox.clone(),
        llm.clone(),
        work_dir.clone(),
        rules.clone(),
    );
    pipeline.run_batch(batch_size, jobs).await?;
    println!("Processing completed.");
    Ok(())
}

async fn execute_sync(
    inbox: &DropboxInbox,
    storage: &Arc<Storage>,
    dropbox: &Arc<dyn DropboxClient>,
) -> Result<(), Error> {
    println!("Syncing from Dropbox folder: '{}'...", inbox.0);
    let entries = dropbox.list_folder(&inbox.0).await?;
    let count = entries.len();
    for entry in entries {
        storage
            .upsert_file(&entry.id, &entry.name, &entry.content_hash)
            .await?;
    }
    println!("{}: Found {} files.", "Sync complete".green(), count);
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
