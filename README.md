# Sci-Librarian

It is a CLI-based automation tool designed to organize scientific articles saved to its inbox directory in Dropbox.

It automates the ingestion of PDFs from a Dropbox Inbox folder, extracts metadata and text using pure Rust libraries,
classifies papers using LLMs based on semantic rules, and archives them into an organized folder structure.

It features a concurrent, state-aware architecture that ensures incremental processing and robust error handling.

It is a single binary, written in Rust, using SQLite and Mistral AI LLM services for categorisation.

## Features and Implementation

See [the specification](./docs/specification.md) for details.

## Quick Start

### Dropbox API Token

You need a Dropbox account and a Dropbox App registration.
Go to [Dropbox Developers](https://www.dropbox.com/developers) and create an app (just for you, you don't need to
publish it). If you name it `Sci-Librarian` it will fit the defaults. Create it with access only to own App folder so
its permissions are limited.

Give it these permissions:
    - `files.metadata.read`  needed to list files
    - `files.content.read`   needed to download files
    - `files.content.write`  needed to upload files to target folders

After creating the app and giving it permssions go to the app `Settings` page under OAuth 2 and generate a new access
token. This will be valid for a few hours allowing you to run the application. If you change permissions, generate a
new token.

Set the `DROPBOX_TOKEN` environment variable to the token value:

```powershell
$env:DROPBOX_TOKEN="secret-token"
``` 

### Mistral AI API Token

Similarly, we need an API key to use the Mistral language model for classifying the articles. Create one in
the [Mistral AI Console](https://console.mistral.ai) (its under _API Keys_) and set the `MISTRAL_API_KEY` environment
variable:

```powershell
$env:MISTRAL_API_KEY="secret-key"
``` 

### Test the Connection

Test the connection:

```powershell
cargo run -- sync 
```

## License

MIT, see [LICENSE](./LICENSE)
