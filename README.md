# Sci-Librarian
It is a CLI-based automation tool designed to organize scientific articles saved to its inbox directory in Dropbox.

It automates the ingestion of PDFs from a Dropbox Inbox folder, extracts metadata and text using pure Rust libraries,
classifies papers using LLMs based on semantic rules, and archives them into an organized folder structure.

It features a concurrent, state-aware architecture that ensures incremental processing and robust error handling.

It is a single binary, written in Rust, using SQLite and OpenRouter LLM services for categorisation.

## Features and Implementation
See [the specification](./docs/specification.md) for details.

## License
MIT, see [LICENSE](./LICENSE)
