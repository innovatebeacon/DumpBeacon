use clap::Parser;

use std::process::exit;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    input: String,

    #[arg(short, long)]
    output: String,

    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let (tx, mut rx) = mpsc::channel(100);
    let cancel_token = CancellationToken::new();

    let input_path = args.input;
    let output_path = args.output;

    let custom_rules = match args.config {
        Some(path) => match dump_beacon::load_rules_from_file(std::path::PathBuf::from(path)) {
            Ok(rules) => Some(rules),
            Err(e) => {
                eprintln!("Error loading config: {}", e);
                exit(1);
            }
        },
        None => None,
    };

    println!("Starting sanitization of {}...", input_path);

    let handle = tokio::spawn(async move {
        let metadata_result = std::fs::metadata(&input_path);
        match metadata_result {
            Ok(metadata) if metadata.is_dir() => {
                dump_beacon::process_directory(input_path, output_path, Some(tx), cancel_token, custom_rules).await
            }
            Ok(_) => {
                dump_beacon::sanitize_sql_file(input_path, output_path, Some(tx), cancel_token, custom_rules).await
            }
            Err(e) => {
                Err(format!("Invalid input path: {}", e))
            }
        }
    });

    while let Some(event) = rx.recv().await {
        match event {
            dump_beacon::ProgressEvent::Percentage(progress) => {
                print!("\rProgress: {:.2}%", progress);
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            dump_beacon::ProgressEvent::BatchStatus(msg) => {
                println!("\n{}", msg);
            }
        }
    }
    println!();

    match handle.await.unwrap() {
        Ok(result) => {
            println!("{}", result.message);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            exit(1);
        }
    }
}
