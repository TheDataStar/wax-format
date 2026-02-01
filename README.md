# WAX (Web Archive eXtended)

> **A high-performance, random-access container format for the offline web.**

WAX is a specialized file format designed to store entire websites (Wikipedia, Documentation, E-Libraries) in a single, highly compressed binary file. Unlike \`.zip\` or \`.tar.gz\`, WAX is optimized for **milliseconds-latency random access**, allowing web servers to stream video and load pages directly from the archive without decompressing the whole file.

## 🏗 Architecture

A \`.wax\` file acts as a **Read-Only File System**. It is composed of three distinct sections:

1.  **The Header (64 bytes)**
    * Contains Magic Bytes (\`WAX1\`), Versioning, and UUIDs.
    * Pads to exactly 64 bytes to align with CPU cache lines for parsing speed.
2.  **The Blob Storage (Body)**
    * Contains the raw file data (HTML, Images, CSS).
    * Each file is compressed individually using **Zstandard (zstd)**.
    * *Why Zstd?* It offers decompression speeds 3-5x faster than Deflate (Zip) with better compression ratios.
3.  **The Index (Footer)**
    * An embedded **SQLite** database appended to the end of the file.
    * Contains the file map: \`path -> (byte_offset, length, mime_type)\`.
    * *Why SQLite?* It allows for complex queries (e.g., "Find all PDFs in the /science folder") instantly, without parsing a proprietary tree structure.

## 📦 Key Dependencies

We rely on a minimal but robust set of Rust crates to ensure stability and performance:

* **\`rusqlite\`**: Used to interface with the embedded SQLite index.
* **\`zstd\`**: The compression engine used by Facebook and the Linux Kernel.
* **\`zerocopy\`**: Allows us to read headers directly from raw memory without expensive data cloning.
* **\`walkdir\`**: Efficiently traverses directory trees during the build process.

## 🚀 Installation

### Prerequisites
* [Rust & Cargo](https://rustup.rs/) (Latest Stable)

### Building form Source
\`\`\`bash
git clone https://github.com/your-org/wax-format.git
cd wax-format
cargo build --release
\`\`\`

The executable will be located in \`./target/release/wax-builder\`.

## 📖 Usage

The \`wax-builder\` CLI handles both the creation and verification of archives.

### 1. Creating an Archive (Build)
Turn a folder of static HTML files into a WAX archive.

\`\`\`bash
# Syntax: wax-builder build --input <SOURCE_FOLDER> --output <DESTINATION_FILE>

cargo run -p wax-builder -- build --input ./wiki-dump --output ./wiki.wax
\`\`\`

> **Note:** The builder automatically detects MIME types (e.g., \`.html\` -> \`text/html\`) and normalizes file paths for cross-platform compatibility.

### 2. Verifying an Archive (Read)
You can inspect the contents of an archive to ensure integrity.

\`\`\`bash
# Syntax: wax-builder read --archive <WAX_FILE> --file <INTERNAL_PATH>

cargo run -p wax-builder -- read --archive ./wiki.wax --file "index.html"
\`\`\`

## ⚠️ Important Considerations

### Static vs. Dynamic Content
WAX is a **static** file container.
* **Works for:** Static HTML, CSS, JS, Images, Videos, SPA (Single Page Apps like React/Vue).
* **Does NOT work for:** PHP, Python, Ruby, or Node.js backend logic.
    * *Example:* If you want to archive a WordPress site, you must first "flatten" it into static HTML using a tool like \`HTTrack\` or \`wget\`.

### Search
While WAX stores the files, it does not inherently "execute" search logic. However, the embedded SQLite index **can** be used by the host OS (like CosmOS) to implement instant full-text search across filenames.

## 🛠 Integrating WAX into Your Rust App

You can use the \`wax-core\` library to add WAX support to your own web server or tools.

\`\`\`toml
# Cargo.toml
[dependencies]
wax-core = { path = "../wax-core" } # Or git url
\`\`\`

\`\`\`rust
use wax_core::reader::WaxReader;

fn serve_request(path: &str) {
    let mut reader = WaxReader::open("library.wax").unwrap();
    
    if let Ok(data) = reader.get_file_data(path) {
        println!("Sending {} bytes...", data.len());
    }
}
\`\`\`

## 📄 License
MIT License. Free for personal and commercial use.
