# fileserve — HTTP file server

A minimal HTTP file server with directory listing, file download, and upload.
Demonstrates async filesystem I/O, path traversal protection, and content negotiation.

```sh
cargo run                       # 127.0.0.1:3333
cargo test                      # filesystem operations
```

## Operations

```sh
# List current directory
curl 127.0.0.1:3333/

# Download a file
curl 127.0.0.1:3333/path/to/file > myfile

# Upload a file
curl -X PUT --data-binary @local.txt 127.0.0.1:3333/uploaded.txt

# List a subdirectory
curl 127.0.0.1:3333/src/
```

## Learning goals

- **Async filesystem** — tokio::fs for non-blocking I/O
- **Path traversal** — validating relative paths with `.starts_with()`
- **Content negotiation** — guessing MIME types from extensions
- **Binary response** — sending non-UTF8 data (images, PDFs, etc.)
- **Error handling** — filesystem errors as HTTP responses
