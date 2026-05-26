# DumpBeacon

DumpBeacon is a production-ready, high-performance database sanitization engine designed to process massive SQL dumps (5GB+) with a negligible memory footprint. 

It provides both a **Standalone CLI** for CI/CD automation and a **Tauri Desktop GUI** for an intuitive user experience.

## Fully Free Forever
DumpBeacon is now completely free to use. There are **no paywalls, no trial limits, and no license keys required.** You get full access to all features out of the box.

### Why it's Free
Data privacy and secure handling of sensitive data shouldn't be locked behind a paywall. We built DumpBeacon to solve the real problem of handling massive database dumps securely without uploading them to the cloud. By open-sourcing our core engine and providing this tool completely free to the developer community, we aim to make secure data handling the standard, not a premium feature.

## Core Features

- **Scale-Independence:** Utilizes a custom two-pass streaming parser. It reads SQL dumps line-by-line without loading the entire file into memory, easily handling 5GB+ or 50GB+ files on standard hardware.
- **Schema-Aware Masking:** Identifies the target schema via `CREATE TABLE` and `INSERT INTO` statements to positionally mask data instead of relying on global regular expressions. This completely eliminates false positives outside of sensitive columns.
- **Multi-Line SQL Support:** The engine seamlessly handles SQL dumps where `VALUES` tuples span multiple lines, maintaining robust state tracking.
- **Real-Time Progress:** Employs an MPSC (Multi-Producer, Single-Consumer) asynchronous channel to stream real-time progress percentages to the CLI console and the Tauri GUI.
- **Graceful Aborts:** Fully integrated cancellation tokens allow you to safely abort the sanitization process mid-stream with automatic cleanup of incomplete files.

## Built With
- **Rust Core:** `tokio` (Async runtime), `regex` (Pattern matching), `tokio-util` (Cancellation tokens).
- **GUI:** `tauri` v2, HTML/JS/Tailwind CSS.
- **CLI:** `clap`.

## Usage

### 1. Tauri GUI
The graphical interface allows you to drag-and-drop a `.sql` file, pick your save destination, and visualize the schema alongside real-time progress.

To run the GUI in development mode:
```bash
npx tauri dev
```
*(Alternatively, use `cargo tauri dev`)*

### 2. Standalone CLI
The CLI is perfect for CI/CD environments or headless servers.

To run the CLI:
```bash
cargo run --bin dump_beacon_cli -- --input <path_to_input.sql> --output <path_to_output.sql>
```

## Security & Privacy
DumpBeacon guarantees **Local-Only Operation**. Telemetry is strictly hard-coded to `false` in the core engine, guaranteeing that your sensitive data and database structures never leave your machine.

## License

This project is licensed under the **MIT License**. See the `LICENSE` file for full details.
