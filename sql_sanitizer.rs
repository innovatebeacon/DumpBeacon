use regex::Regex;
use serde::Deserialize;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

#[derive(Deserialize, Debug)]
pub struct CustomRule {
    pub name: String,
    pub pattern: String,
    pub placeholder: String,
}

/// Sanitizes an SQL file by reading it line-by-line asynchronously and replacing
/// sensitive information such as emails, phone numbers, and passwords with safe mock values.
/// This streaming approach ensures memory efficiency even for multi-gigabyte files.
///
/// # Arguments
///
/// * `input_path` - The file path to the source SQL file.
/// * `output_path` - The file path where the sanitized SQL file will be written.
///
/// # Returns
///
/// A `Result` containing a success summary message or an error message as a `String`.
pub async fn sanitize_sql_file(input_path: String, output_path: String) -> Result<String, String> {
    // Open the input file for asynchronous reading
    let input_file = File::open(&input_path)
        .await
        .map_err(|e| format!("Failed to open input file '{}': {}", input_path, e))?;
    let mut reader = BufReader::new(input_file);

    // Open the output file for asynchronous writing, creating or truncating it
    let output_file = File::create(&output_path)
        .await
        .map_err(|e| format!("Failed to create output file '{}': {}", output_path, e))?;
    let mut writer = BufWriter::new(output_file);

    // --- Compile Regex Pattern Matchers ---
    
    // Matches standard email formats globally within any string segment
    let email_regex = Regex::new(r"([a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,})")
        .map_err(|e| format!("Failed to compile email regex: {}", e))?;

    // Matches 'password', 'pass', 'hash', or 'pwd' followed by an equals sign or colon, and captures the quoted value
    // e.g., password='my_secret_password' or "hash": "abc123def"
    // Capture groups:
    // 1: The indicator keyword
    // 2: The structural characters between keyword and the quote (e.g., ' = ' or '": ')
    // 3: The opening quote character (' or ")
    // 4: The sensitive value being masked
    // 5: The closing quote character (' or ")
    let password_regex = Regex::new(r#"(?i)(password|pass|hash|pwd)([^'"]*?)(['"])([^'"]+)(['"])"#)
        .map_err(|e| format!("Failed to compile password regex: {}", e))?;

    // Matches phone number indicators and captures the number inside quotes
    // e.g., 'phone': '+1-555-123-4567'
    let phone_regex = Regex::new(r#"(?i)(phone|tel|mobile)([^'"]*?)(['"])([\+\d\-\(\)\s]{7,20})(['"])"#)
        .map_err(|e| format!("Failed to compile phone regex: {}", e))?;

    // Load custom dynamic rules if the configuration file exists
    let mut custom_rules = Vec::new();
    if let Ok(rules_content) = std::fs::read_to_string("rules.json") {
        match serde_json::from_str::<Vec<CustomRule>>(&rules_content) {
            Ok(rules) => custom_rules = rules,
            Err(e) => return Err(format!("Failed to parse rules.json: {}", e)),
        }
    }

    // --- Streaming Execution ---

    // A reusable buffer to minimize allocation overhead per line
    let mut line = String::new();
    let mut lines_processed: u64 = 0;
    let mut data_masked: u64 = 0;

    // Stream the file line-by-line
    loop {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("Error reading line {}: {}", lines_processed + 1, e))?;

        // End of file
        if bytes_read == 0 {
            break;
        }

        let mut sanitized_line = line.clone();
        let mut modified = false;

        // 1. Mask Emails
        if email_regex.is_match(&sanitized_line) {
            sanitized_line = email_regex.replace_all(&sanitized_line, "user_masked@example.com").to_string();
            modified = true;
        }

        // 2. Mask Passwords / Hashes
        if password_regex.is_match(&sanitized_line) {
            // Reconstruct the string keeping the structural layout intact but swapping the value
            sanitized_line = password_regex.replace_all(&sanitized_line, "${1}${2}${3}[MASKED_HASH]${5}").to_string();
            modified = true;
        }

        // 3. Mask Phone Numbers
        if phone_regex.is_match(&sanitized_line) {
            sanitized_line = phone_regex.replace_all(&sanitized_line, "${1}${2}${3}[MASKED_PHONE]${5}").to_string();
            modified = true;
        }

        // 4. Apply Dynamic Custom Rules
        for rule in &custom_rules {
            if let Ok(dynamic_regex) = Regex::new(&rule.pattern) {
                if dynamic_regex.is_match(&sanitized_line) {
                    sanitized_line = dynamic_regex.replace_all(&sanitized_line, &rule.placeholder).to_string();
                    modified = true;
                }
            }
        }

        // Track masking statistics
        if modified {
            data_masked += 1;
        }

        // Stream the sanitized line immediately to the output file
        writer
            .write_all(sanitized_line.as_bytes())
            .await
            .map_err(|e| format!("Error writing to output file on line {}: {}", lines_processed + 1, e))?;
            
        lines_processed += 1;
    }

    // Ensure all remaining buffered data is flushed to disk
    writer
        .flush()
        .await
        .map_err(|e| format!("Failed to flush output file buffer: {}", e))?;

    Ok(format!(
        "Sanitization complete! Processed {} lines and successfully masked sensitive data in {} rows.",
        lines_processed, data_masked
    ))
}
