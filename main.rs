mod sql_sanitizer;

#[tokio::main]
async fn main() {
    // Relative path to look for sample.sql (e.g., in the parent directory)
    let input_path = "../sample.sql".to_string();
    let output_path = "sanitized_sample.sql".to_string();

    println!("--------------------------------------------------");
    println!(" DumpBeacon SQL Sanitizer ");
    println!("--------------------------------------------------");
    println!("Scanning input file: {}", input_path);
    println!("Writing sanitized output to: {}", output_path);
    println!("Processing...");

    // Execute the async sanitizer function
    match sql_sanitizer::sanitize_sql_file(input_path, output_path).await {
        Ok(metrics_summary) => {
            println!("\n[SUCCESS] {}", metrics_summary);
        }
        Err(error_msg) => {
            eprintln!("\n[ERROR] {}", error_msg);
        }
    }
}
