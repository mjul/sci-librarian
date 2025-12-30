# Sci-Librarian
It is a CLI-based automation tool designed to organize scientific articles saved to its inbox directory in Dropbox.

It automates the ingestion of PDFs from a Dropbox Inbox folder, extracts metadata and text using pure Rust libraries,
classifies papers using LLMs based on semantic rules, and archives them into an organized folder structure.

It features a concurrent, state-aware architecture that ensures incremental processing and robust error handling.

It is a single binary, written in Rust, using SQLite and OpenRouter LLM services for categorisation.

## Features and Implementation
See [the specification](./docs/specification.md) for details.

## Quick Start

### Dropbox API Token
You need a Dropbox account. Go to Developers and create an app (its just for you), then on its `Settings` page
under OAuth 2, generate a new access token. This will be valid for a few hours allowing you to run the application.

Set the `DROPBOX_TOKEN` environment variable to the token value.

```powershell
$env:DROPBOX_TOKEN="secret-token"
``` 

### OpenRouter API Token
```powershell
$env:OPENROUTER_API_KEY="secret-key"
``` 

### Test the Connection
Test the connection: 

```powershell
cargo run -- sync 
```

## License
MIT, see [LICENSE](./LICENSE)
