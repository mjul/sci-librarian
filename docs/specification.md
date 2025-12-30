# Product Requirements Document: Sci-Librarian

| **Project Name** | sci-librarian                                    |
| :-- |:-------------------------------------------------|
| **Version** | 1.0 (Draft)                                      |
| **Date** | December 30, 2025                                |
| **Status** | Approved for Development                         |
| **Tech Stack** | Rust (Pure), SQLite, Dropbox API, OpenRouter LLM |

## 1. Executive Summary

**sci-librarian** is a CLI-based automation tool designed to organize scientific articles saved to its inbox directory in Dropbox. 

It automates the ingestion of PDFs from a Dropbox Inbox folder, extracts metadata and text using pure Rust libraries, 
classifies papers using LLMs based on semantic rules, and archives them into an organized folder structure. 

It features a concurrent, state-aware architecture that ensures incremental processing and robust error handling.

## 2. Core Philosophy

* **Local-First \& Idempotent:** All state is tracked locally in SQLite. The tool can be stopped and restarted without losing progress or duplicating work.
* **Pure Rust \& Self-Contained:** A single binary executable with no heavy runtime dependencies (no Docker, no Python, no Poppler).
* **Strictly Typed:** Uses semantic types (newtypes) to prevent logic errors (_e.g._, distinguishing a `DropboxId` from a `FileHash`).
* **Concurrency:** Efficient parallel processing of file batches with a unified UI.

## 3. Functional Workflows

### 3.1. Ingestion (`sync`)

* **Input:** Dropbox `/Inbox` folder.
* **Concurrency:** Single threaded.
* **Logic:**

1. List files in `/Inbox` via Dropbox API `list_folder` operation with paging.
2. Compare file content hashes against the local `state.db`.
3. **State Update:** Add new or modified files and their hashes to `state.db`, mark for processing.  

### 3.1.B Download (`get-batch`)
* **Input:** `state.db`
* **Concurrency:** Single threaded.
* **Logic:**
1. Select a batch of the newest pending files from `state.db`
2. Select maximum `--batch-size {integer}` files in one operation (default: 10).
3. Invoke `get-file` (see below) for each
 
### 3.1.C. Download (`get-file`)
* **Input:** Dropbox file ID
* **Concurrency:** Single threaded.
* **Logic:**
1. Download a batch of pending files sequentially to a local `./working/raw/` directory.
3. **State Update:** Update the local `state.db` state for the file after it is downloaded.


### 3.2 Processing (`process`)
Control loop for processing pending files concurrently.

* **Input:** Dropbox file ID
* **Concurrency:** Single threaded.
* **Logic:**
1. Select pending files from `state.db`
2. For each file, place file information on Work Queue for processing (see `process-file` below).
3. Collect results from worker threads.
4. **State Update:** As results from `process-file` arrive, mark file as `Processed` in `state.db`; record `target_path`, record `summary`, record `abstract` (extracted title, authors and abstract).
 
### 3.2.B Processing (`process-file`)
Process a single file as a pure function. 

* **Input:** file information (from `state.db`), Rules tuples, and a Local PDF file.
* **Concurrency:** Configurable via `-j` (default: 4). One thread per file.
* **Logic (Per File):**
1. **Extraction:** Use `lopdf` to extract raw text strings from the first ~5 pages.
2. **Analysis (LLM):** Send text to an LLM (_e.g._, Gemini Flash).
   * *Prompt:* "Extract Title, Authors, Abstract. Provide a 1-line summary. Match abstract against provided Rules to select a Target Path."
3. **Return:** `target_path` array (possibly empty), extracted metadata (summary, title, authors, abstract).

### 3.2.C Upload (`upload`)
* **Trigger:** Runs automatically at end of `process` batch for any processed (but not uploaded) files. 
* **Input:** file information (from `state.db`) with target paths and metadata, local PDF file.
* **Concurrency:** Single threaded.
* **Logic (Per File):**
1. **Upload:**
   * For each target directory:
     * Upload the original PDF to the `Target Path` in Dropbox.
     * Upload a sidecar Markdown file (`{filename}.md`) containing metadata and extracted text.

### 3.3. Indexing (`index`)

* **Trigger:** Runs automatically at the end of a `process` batch for any "touched" folders, or manually via CLI.
* **Concurrency:** Single threaded.
* **Logic:**

1. Query DB for all archived files in specific target directories.
2. Generate a `README.md` containing a Markdown table:
   * Columns: `Title` (linked to PDF), `Authors`, `One-Line Summary`.
3. Upload/Overwrite `README.md` in the respective Dropbox folder.


## 4. Technical Architecture

### 4.1. The Fan-Out / Fan-In Pattern

To ensure SQLite integrity and UI stability, the system uses a **Producer-Consumer** model.

1. **Scanner:** Pushes `Job` structs to a generic queue.
2. **Workers (xN):** Pick up jobs, perform I/O (Download -> Extract -> LLM -> Upload), and send `JobResult` to a result queue. **No DB writes happen here.**
3. **Collector (Actor):** The single owner of the SQLite connection and Progress Bar. It listens to the result queue, writes success/failure to the DB, and updates the UI.

### 4.2. Data Models (Strict Types)

We use the Newtype pattern to enforce semantic safety.

```rust
pub struct DropboxId(String);
pub struct RemotePath(String);
pub struct LocalPath(PathBuf);
pub struct OneLineSummary(String);

pub struct Job {
    pub id: DropboxId,
    pub path: RemotePath,
}

pub enum JobResult {
    Success { id: DropboxId, meta: ArticleMetadata, target: RemotePath },
    Failure { id: DropboxId, error: String },
}
```


### 4.3. Database Schema (SQLite)

```sql
CREATE TABLE files (
    dropbox_id TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    status TEXT NOT NULL,       -- 'PENDING', 'ARCHIVED'
    title TEXT,
    authors TEXT,               -- JSON array
    summary TEXT,               -- Cached for README generation
    target_path TEXT,           -- For indexing lookups
    last_error TEXT,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_folder ON files(target_path);
```


## 5. Technology Stack

| Component | Choice | Justification |
| :-- | :-- | :-- |
| **Language** | **Rust (2021)** | Performance, safety, single binary distribution. |
| **CLI** | **`clap`** | Standard, robust argument parsing. |
| **Async Runtime** | **`tokio`** | Required for concurrent I/O. |
| **HTTP Client** | **`reqwest`** | Robust handling of Dropbox/LLM APIs. |
| **PDF Engine** | **`lopdf`** | Pure Rust extraction. No system deps. |
| **Database** | **`sqlx` (SQLite)** | Async, compile-time checked queries (optional). |
| **UI/Progress** | **`indicatif`** | Rich progress bars and spinners. |
| **State Sync** | **`tokio::sync::mpsc`** | Message passing for the Collector pattern. |

## 6. CLI Command Structure

```bash
# Standard run: Syncs, processes (4 threads), and re-indexes changed folders
$ sci-librarian run -j 4 

# Only download new files
$ sci-librarian sync

# Only process downloaded files (useful if offline previously)
$ sci-librarian process --jobs 8

# Force regeneration of index for a specific topic
$ sci-librarian index --path "/Research/Quantum_Computing"
```


## 7. Configuration 
- API keys and secrets for Dropbox and Openrouter are given via environment variables. Validate that these exist on program start.
- Batch size is given by CLI parameter (default: 10)
- Rules file is in `rules.yaml`, or specified via `--rules {filename}` parameter. 


## 8. Development Roadmap

1. **Phase 1: Foundation.** Setup `clap`, `config`, and Dropbox `sync` (list \& download). Setup SQLite migrations.
2. **Phase 2: The Pipeline.** Implement `lopdf` extraction and the OpenRouter client. Create the `Job` / `Result` message structures.
3. **Phase 3: The Actor.** Implement the Collector loop and `indicatif` integration. Wire up the parallel workers.
4. **Phase 4: Indexing.** Implement the `README.md` generator and upload logic.
5. **Phase 5: Refinement.** Add retry logic for network flakes and robust error reporting.

## 9. Constraints \& Out of Scope

* **OCR:** We will not perform OCR on image-only PDFs. If `lopdf` extracts no text, the file will be flagged as `Skipped/ImageOnly`.
* **Perfect Layout:** Markdown output will be raw text; we do not attempt to preserve complex multi-column layouts visually, as the LLM handles the semantic reconstruction.

