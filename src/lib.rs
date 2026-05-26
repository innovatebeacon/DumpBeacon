use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs::{metadata, File};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", content = "payload")]
pub enum ProgressEvent {
    Percentage(f64),
    BatchStatus(String),
}

pub const TELEMETRY_ENABLED: bool = false;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct CustomRule {
    pub name: String,
    pub pattern: String,
    pub placeholder: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ColumnRule {
    pub name: String,
    pub keywords: Vec<String>,
    pub placeholder: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SanitizationRules {
    pub column_rules: Vec<ColumnRule>,
    pub regex_rules: Vec<CustomRule>,
}

impl Default for SanitizationRules {
    fn default() -> Self {
        Self {
            column_rules: vec![
                ColumnRule {
                    name: "Password".to_string(),
                    keywords: vec![
                        "password".into(),
                        "pass".into(),
                        "passwd".into(),
                        "pwd".into(),
                        "hash".into(),
                    ],
                    placeholder: "[MASKED_HASH]".to_string(),
                },
                ColumnRule {
                    name: "Phone".to_string(),
                    keywords: vec![
                        "phone".into(),
                        "mobile".into(),
                        "telephone".into(),
                        "tel".into(),
                    ],
                    placeholder: "[MASKED_PHONE]".to_string(),
                },
                ColumnRule {
                    name: "Email".to_string(),
                    keywords: vec!["email".into(), "e_mail".into(), "mail".into()],
                    placeholder: "user_masked@example.com".to_string(),
                },
                ColumnRule {
                    name: "Credit Card".to_string(),
                    keywords: vec!["card".into(), "cc".into(), "credit".into()],
                    placeholder: "[MASKED_CARD]".to_string(),
                },
            ],
            regex_rules: vec![],
        }
    }
}

impl SanitizationRules {
    pub fn get_placeholder(&self, column_name: &str) -> Option<&str> {
        let name = column_name.to_lowercase();
        for rule in &self.column_rules {
            for keyword in &rule.keywords {
                if name.contains(keyword) {
                    return Some(&rule.placeholder);
                }
            }
        }
        None
    }
}

pub fn load_rules_from_file(path: PathBuf) -> Result<SanitizationRules, String> {
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read rules file at {:?}: {}", path, e))?;
    let rules: SanitizationRules = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse rules file: {}", e))?;
    Ok(rules)
}

#[derive(Serialize, Debug)]
pub struct SanitizerResult {
    pub message: String,
    pub schema: HashMap<String, Vec<String>>,
}

fn normalize_line_for_matching(bytes: &[u8]) -> String {
    let mut line = String::from_utf8_lossy(bytes).into_owned();
    if line.contains('\0') {
        line.retain(|c| c != '\0' && c != '\u{feff}');
    } else if line.starts_with('\u{feff}') {
        line = line.trim_start_matches('\u{feff}').to_string();
    }
    line
}

fn clean_sql_identifier(identifier: &str) -> String {
    let cleaned = identifier
        .trim()
        .trim_end_matches(',')
        .replace(&['`', '"', '[', ']'][..], "");

    cleaned
        .split('.')
        .last()
        .unwrap_or(cleaned.as_str())
        .trim()
        .to_string()
}

fn parse_column_name(line: &str, column_regex: &Regex) -> Option<String> {
    let caps = column_regex.captures(line)?;
    let raw_name = (1..=4).find_map(|idx| caps.get(idx).map(|m| m.as_str()))?;
    let column_name = clean_sql_identifier(raw_name);
    let upper_name = column_name.to_uppercase();
    let skip_words = [
        "CONSTRAINT", "PRIMARY", "FOREIGN", "UNIQUE", "KEY", "INDEX", "CHECK", "EXCLUDE",
        "PARTITION", "LIKE",
    ];

    if skip_words.contains(&upper_name.as_str()) {
        None
    } else {
        Some(column_name)
    }
}

fn split_sql_identifiers(list: &str) -> Vec<String> {
    list.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .map(clean_sql_identifier)
        .filter(|item| !item.is_empty())
        .collect()
}



fn mask_insert_values(
    values_sql: &str,
    columns: &[String],
    rules: &SanitizationRules,
) -> (String, bool) {
    let sensitive_columns: HashMap<usize, &str> = columns
        .iter()
        .enumerate()
        .filter_map(|(idx, col)| rules.get_placeholder(col).map(|placeholder| (idx, placeholder)))
        .collect();

    if sensitive_columns.is_empty() {
        return (values_sql.to_string(), false);
    }

    let mut output = String::with_capacity(values_sql.len());
    let mut chars = values_sql.chars().peekable();
    let mut tuple_depth = 0usize;
    let mut value_index = 0usize;
    let mut modified = false;

    while let Some(ch) = chars.next() {
        match ch {
            '(' if tuple_depth == 0 => {
                tuple_depth = 1;
                value_index = 0;
                output.push(ch);
            }
            '(' => {
                tuple_depth += 1;
                output.push(ch);
            }
            ')' if tuple_depth > 0 => {
                tuple_depth -= 1;
                output.push(ch);
            }
            ',' if tuple_depth == 1 => {
                value_index += 1;
                output.push(ch);
            }
            '\'' | '"' if tuple_depth >= 1 => {
                if let Some(placeholder) = sensitive_columns.get(&value_index) {
                    let quote = ch;
                    output.push(quote);
                    output.push_str(placeholder);
                    output.push(quote);
                    modified = true;

                    while let Some(inner) = chars.next() {
                        if inner == '\\' {
                            let _ = chars.next();
                            continue;
                        }

                        if inner == quote {
                            if chars.peek() == Some(&quote) {
                                let _ = chars.next();
                                continue;
                            }
                            break;
                        }
                    }
                } else {
                    let quote = ch;
                    output.push(ch);

                    while let Some(inner) = chars.next() {
                        output.push(inner);

                        if inner == '\\' {
                            if let Some(escaped) = chars.next() {
                                output.push(escaped);
                            }
                            continue;
                        }

                        if inner == quote {
                            if chars.peek() == Some(&quote) {
                                if let Some(escaped_quote) = chars.next() {
                                    output.push(escaped_quote);
                                }
                                continue;
                            }
                            break;
                        }
                    }
                }
            }
            _ => output.push(ch),
        }
    }

    (output, modified)
}



fn infer_schema_from_insert_line(
    line: &str,
    insert_regex: &Regex,
) -> Option<(String, Vec<String>)> {
    let caps = insert_regex.captures(line)?;
    let table_name = clean_sql_identifier(caps.name("table")?.as_str());
    let columns = caps
        .name("columns")
        .map(|explicit_columns| split_sql_identifiers(explicit_columns.as_str()))
        .unwrap_or_default();

    if table_name.is_empty() || columns.is_empty() {
        None
    } else {
        Some((table_name, columns))
    }
}

pub async fn sanitize_sql_file(
    input_path: String,
    output_path: String,
    progress_tx: Option<mpsc::Sender<ProgressEvent>>,
    cancel_token: CancellationToken,
    custom_rules: Option<SanitizationRules>,
) -> Result<SanitizerResult, String> {
    let file_meta = metadata(&input_path)
        .await
        .map_err(|e| format!("Failed to get file metadata: {}", e))?;
    let total_bytes = file_meta.len() as f64;
    
    let sql_identifier = r#"(?:`[^`]+`|"[^"]+"|\[[^\]]+\]|[A-Za-z_][A-Za-z0-9_$]*)(?:\s*\.\s*(?:`[^`]+`|"[^"]+"|\[[^\]]+\]|[A-Za-z_][A-Za-z0-9_$]*))*"#;
    let create_table_regex = Regex::new(&format!(
        r#"(?i)^\s*CREATE\s+(?:TEMP(?:ORARY)?\s+|UNLOGGED\s+)?TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?({})"#,
        sql_identifier
    ))
    .map_err(|e| format!("Failed to compile create table regex: {}", e))?;
    let column_regex =
        Regex::new(r#"^\s*(?:`([^`]+)`|"([^"]+)"|\[([^\]]+)\]|([A-Za-z_][A-Za-z0-9_$]*))\s+"#)
            .map_err(|e| format!("Failed to compile column regex: {}", e))?;
    let insert_regex = Regex::new(&format!(
        r#"(?is)^(?P<head>.*?\bINSERT\s+INTO\s+(?P<table>{})\s*)(?P<columns>\([^)]*\))?(?P<values_keyword>\s+VALUES\s*)(?P<values>.*)$"#,
        sql_identifier
    ))
    .map_err(|e| format!("Failed to compile insert regex: {}", e))?;

    let rules = custom_rules.unwrap_or_default();

    let mut schema: HashMap<String, Vec<String>> = HashMap::new();
    let mut current_table: Option<String> = None;
    let mut in_table_definition = false;
    let mut bytes_read_total = 0u64;
    let mut last_reported_progress = 0.0;

    let mut buf = Vec::new();
    {
        let input_file = File::open(&input_path)
            .await
            .map_err(|e| format!("Failed to open input file '{}': {}", input_path, e))?;
        let mut reader = BufReader::new(input_file);

        loop {
            if cancel_token.is_cancelled() {
                return Err("Job cancelled".to_string());
            }

            buf.clear();
            let bytes_read = reader
                .read_until(b'\n', &mut buf)
                .await
                .map_err(|e| format!("Error reading in pass 1: {}", e))?;

            if bytes_read == 0 {
                break;
            }

            bytes_read_total += bytes_read as u64;

            if bytes_read_total % 10_485_760 < bytes_read as u64 {
                let current_progress = (bytes_read_total as f64 / total_bytes) * 50.0;
                if current_progress - last_reported_progress >= 1.0 {
                    if let Some(tx) = &progress_tx {
                        let _ = tx.send(ProgressEvent::Percentage(current_progress)).await;
                    }
                    last_reported_progress = current_progress;
                }
            }

            let line = normalize_line_for_matching(&buf);
            let line_trimmed = line.trim();
            if !line_trimmed.is_empty()
                && !line_trimmed.starts_with("--")
                && !line_trimmed.starts_with("/*")
            {
                if let Some(caps) = create_table_regex.captures(line_trimmed) {
                    let table_name = clean_sql_identifier(caps.get(1).unwrap().as_str());
                    schema.insert(table_name.clone(), Vec::new());
                    current_table = Some(table_name);
                    in_table_definition = true;
                } else if in_table_definition {
                    if line_trimmed.starts_with(')') || line_trimmed.starts_with('}') {
                        in_table_definition = false;
                        current_table = None;
                    } else {
                        let upper_line = line_trimmed.to_uppercase();
                        if !upper_line.starts_with("PRIMARY")
                            && !upper_line.starts_with("FOREIGN")
                            && !upper_line.starts_with("UNIQUE")
                            && !upper_line.starts_with("KEY")
                            && !upper_line.starts_with("INDEX")
                            && !upper_line.starts_with("CONSTRAINT")
                            && !upper_line.starts_with("CHECK")
                        {
                            if let Some(col_name) = parse_column_name(line_trimmed, &column_regex) {
                                if let Some(table_name) = &current_table {
                                    if let Some(cols) = schema.get_mut(table_name) {
                                        if !cols.contains(&col_name) {
                                            cols.push(col_name);
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if let Some((table_name, columns)) =
                    infer_schema_from_insert_line(line_trimmed, &insert_regex)
                {
                    schema.entry(table_name).or_insert(columns);
                }
            }
        }
    }

    let mut lines_processed: u64 = 0;
    let mut data_masked: u64 = 0;
    bytes_read_total = 0;
    last_reported_progress = 50.0;

    let mut active_insert: Option<(String, Vec<String>)> = None;

    {
        let input_file = File::open(&input_path)
            .await
            .map_err(|e| format!("Failed to open input file '{}': {}", input_path, e))?;
        let mut reader = BufReader::new(input_file);

        let output_file = File::create(&output_path)
            .await
            .map_err(|e| format!("Failed to create output file '{}': {}", output_path, e))?;
        let mut writer = BufWriter::new(output_file);

        loop {
            if cancel_token.is_cancelled() {
                let _ = tokio::fs::remove_file(&output_path).await;
                return Err("Job cancelled".to_string());
            }

            buf.clear();
            let bytes_read = reader
                .read_until(b'\n', &mut buf)
                .await
                .map_err(|e| format!("Error reading line {}: {}", lines_processed + 1, e))?;

            if bytes_read == 0 {
                break;
            }

            bytes_read_total += bytes_read as u64;

            if bytes_read_total % 10_485_760 < bytes_read as u64 {
                let current_progress = 50.0 + (bytes_read_total as f64 / total_bytes) * 50.0;
                if current_progress - last_reported_progress >= 1.0 {
                    if let Some(tx) = &progress_tx {
                        let _ = tx.send(ProgressEvent::Percentage(current_progress)).await;
                    }
                    last_reported_progress = current_progress;
                }
            }

            let line = normalize_line_for_matching(&buf);
            let mut sanitized_line = line.clone();
            let mut modified = false;

            if let Some(caps) = insert_regex.captures(&sanitized_line) {
                let table_name = clean_sql_identifier(caps.name("table").unwrap().as_str());
                let columns = if let Some(explicit_columns) = caps.name("columns") {
                    split_sql_identifiers(explicit_columns.as_str())
                } else {
                    schema.get(&table_name).cloned().unwrap_or_default()
                };

                if !columns.is_empty() {
                    let values = caps.name("values").unwrap().as_str();
                    let (masked_values, did_modify) = mask_insert_values(values, &columns, &rules);
                    
                    if did_modify {
                        sanitized_line = format!(
                            "{}{}{}{}",
                            caps.name("head").map(|m| m.as_str()).unwrap_or(""),
                            caps.name("columns").map(|m| m.as_str()).unwrap_or(""),
                            caps.name("values_keyword").map(|m| m.as_str()).unwrap_or(""),
                            masked_values
                        );
                        modified = true;
                    }

                    if !sanitized_line.trim_end().ends_with(';') {
                        active_insert = Some((table_name, columns));
                    } else {
                        active_insert = None;
                    }
                } else {
                    active_insert = None;
                }
            } else if let Some((_table, columns)) = &active_insert {
                let (masked_values, did_modify) = mask_insert_values(&sanitized_line, columns, &rules);
                if did_modify {
                    sanitized_line = masked_values;
                    modified = true;
                }
                
                if sanitized_line.trim_end().ends_with(';') {
                    active_insert = None;
                }
            }

            for rule in &rules.regex_rules {
                if let Ok(dynamic_regex) = Regex::new(&rule.pattern) {
                    if dynamic_regex.is_match(&sanitized_line) {
                        sanitized_line = dynamic_regex
                            .replace_all(&sanitized_line, &rule.placeholder)
                            .to_string();
                        modified = true;
                    }
                }
            }

            if modified {
                data_masked += 1;
            }

            writer
                .write_all(sanitized_line.as_bytes())
                .await
                .map_err(|e| {
                    format!(
                        "Error writing to output file on line {}: {}",
                        lines_processed + 1,
                        e
                    )
                })?;

            lines_processed += 1;
        }

        writer
            .flush()
            .await
            .map_err(|e| format!("Failed to flush output file buffer: {}", e))?;
    }

    if let Some(tx) = &progress_tx {
        let _ = tx.send(ProgressEvent::Percentage(100.0)).await;
    }

    Ok(SanitizerResult {
        message: format!(
            "Sanitization complete! Processed {} lines and successfully masked sensitive data in {} rows.",
            lines_processed, data_masked
        ),
        schema,
    })
}

pub async fn preview_sanitize(
    input_path: String,
    limit: usize,
    custom_rules: Option<SanitizationRules>,
) -> Result<Vec<(String, String)>, String> {
    let rules = custom_rules.unwrap_or_default();
    
    let sql_identifier = r#"(?:`[^`]+`|"[^"]+"|\[[^\]]+\]|[A-Za-z_][A-Za-z0-9_$]*)(?:\s*\.\s*(?:`[^`]+`|"[^"]+"|\[[^\]]+\]|[A-Za-z_][A-Za-z0-9_$]*))*"#;
    let create_table_regex = Regex::new(&format!(
        r#"(?i)^\s*CREATE\s+(?:TEMP(?:ORARY)?\s+|UNLOGGED\s+)?TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?({})"#,
        sql_identifier
    )).unwrap();
    let column_regex =
        Regex::new(r#"^\s*(?:`([^`]+)`|"([^"]+)"|\[([^\]]+)\]|([A-Za-z_][A-Za-z0-9_$]*))\s+"#).unwrap();
    let insert_regex = Regex::new(&format!(
        r#"(?is)^(?P<head>.*?\bINSERT\s+INTO\s+(?P<table>{})\s*)(?P<columns>\([^)]*\))?(?P<values_keyword>\s+VALUES\s*)(?P<values>.*)$"#,
        sql_identifier
    )).unwrap();

    let mut schema: HashMap<String, Vec<String>> = HashMap::new();
    let mut current_table: Option<String> = None;
    let mut in_table_definition = false;
    let mut active_insert: Option<(String, Vec<String>)> = None;

    let mut results = Vec::new();
    
    let input_file = File::open(&input_path)
        .await
        .map_err(|e| format!("Failed to open input file '{}': {}", input_path, e))?;
    let mut reader = BufReader::new(input_file);
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buf)
            .await
            .map_err(|e| format!("Error reading line: {}", e))?;

        if bytes_read == 0 {
            break;
        }

        let line = normalize_line_for_matching(&buf);
        let line_trimmed = line.trim();
        let mut sanitized_line = line.clone();
        let mut modified = false;
        
        // Pass 1 Schema Logic
        if !line_trimmed.is_empty()
            && !line_trimmed.starts_with("--")
            && !line_trimmed.starts_with("/*")
        {
            if let Some(caps) = create_table_regex.captures(line_trimmed) {
                let table_name = clean_sql_identifier(caps.get(1).unwrap().as_str());
                schema.insert(table_name.clone(), Vec::new());
                current_table = Some(table_name);
                in_table_definition = true;
            } else if in_table_definition {
                if line_trimmed.starts_with(')') || line_trimmed.starts_with('}') {
                    in_table_definition = false;
                    current_table = None;
                } else {
                    let upper_line = line_trimmed.to_uppercase();
                    if !upper_line.starts_with("PRIMARY")
                        && !upper_line.starts_with("FOREIGN")
                        && !upper_line.starts_with("UNIQUE")
                        && !upper_line.starts_with("KEY")
                        && !upper_line.starts_with("INDEX")
                        && !upper_line.starts_with("CONSTRAINT")
                        && !upper_line.starts_with("CHECK")
                    {
                        if let Some(col_name) = parse_column_name(line_trimmed, &column_regex) {
                            if let Some(table_name) = &current_table {
                                if let Some(cols) = schema.get_mut(table_name) {
                                    if !cols.contains(&col_name) {
                                        cols.push(col_name);
                                    }
                                }
                            }
                        }
                    }
                }
            } else if let Some((table_name, columns)) =
                infer_schema_from_insert_line(line_trimmed, &insert_regex)
            {
                schema.entry(table_name).or_insert(columns);
            }
        }
        
        // Pass 2 Masking Logic
        if let Some(caps) = insert_regex.captures(&sanitized_line) {
            let table_name = clean_sql_identifier(caps.name("table").unwrap().as_str());
            let columns = if let Some(explicit_columns) = caps.name("columns") {
                split_sql_identifiers(explicit_columns.as_str())
            } else {
                schema.get(&table_name).cloned().unwrap_or_default()
            };

            if !columns.is_empty() {
                let values = caps.name("values").unwrap().as_str();
                let (masked_values, did_modify) = mask_insert_values(values, &columns, &rules);
                
                if did_modify {
                    sanitized_line = format!(
                        "{}{}{}{}",
                        caps.name("head").map(|m| m.as_str()).unwrap_or(""),
                        caps.name("columns").map(|m| m.as_str()).unwrap_or(""),
                        caps.name("values_keyword").map(|m| m.as_str()).unwrap_or(""),
                        masked_values
                    );
                    modified = true;
                }

                if !sanitized_line.trim_end().ends_with(';') {
                    active_insert = Some((table_name, columns));
                } else {
                    active_insert = None;
                }
            } else {
                active_insert = None;
            }
        } else if let Some((_table, columns)) = &active_insert {
            let (masked_values, did_modify) = mask_insert_values(&sanitized_line, columns, &rules);
            if did_modify {
                sanitized_line = masked_values;
                modified = true;
            }
            
            if sanitized_line.trim_end().ends_with(';') {
                active_insert = None;
            }
        }

        for rule in &rules.regex_rules {
            if let Ok(dynamic_regex) = Regex::new(&rule.pattern) {
                if dynamic_regex.is_match(&sanitized_line) {
                    sanitized_line = dynamic_regex
                        .replace_all(&sanitized_line, &rule.placeholder)
                        .to_string();
                    modified = true;
                }
            }
        }
        
        if modified {
            results.push((line.clone(), sanitized_line.clone()));
            if results.len() >= limit {
                break;
            }
        }
    }
    
    Ok(results)
}

pub async fn process_directory(
    input_dir: String,
    output_dir: String,
    progress_tx: Option<mpsc::Sender<ProgressEvent>>,
    cancel_token: CancellationToken,
    custom_rules: Option<SanitizationRules>,
) -> Result<SanitizerResult, String> {
    let mut entries = tokio::fs::read_dir(&input_dir).await.map_err(|e| e.to_string())?;
    let mut sql_files = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let path = entry.path();
        if path.is_file() && path.extension().map_or(false, |ext| ext == "sql") {
            sql_files.push(path);
        }
    }

    let total_files = sql_files.len();
    if total_files == 0 {
        return Err("No .sql files found in directory".into());
    }

    let mut combined_schema = HashMap::new();

    for (i, file_path) in sql_files.iter().enumerate() {
        if cancel_token.is_cancelled() {
            return Err("Sanitization cancelled".into());
        }

        let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
        if let Some(tx) = &progress_tx {
            let _ = tx.send(ProgressEvent::BatchStatus(format!("Processing {} of {}: {}", i + 1, total_files, filename))).await;
            let _ = tx.send(ProgressEvent::Percentage(0.0)).await;
        }

        let out_path = std::path::Path::new(&output_dir).join(&filename);
        
        let result = sanitize_sql_file(
            file_path.to_string_lossy().to_string(),
            out_path.to_string_lossy().to_string(),
            progress_tx.clone(),
            cancel_token.clone(),
            custom_rules.clone()
        ).await?;

        for (k, v) in result.schema {
            combined_schema.insert(k, v);
        }
    }

    Ok(SanitizerResult {
        message: format!("Batch sanitization complete! Processed {} files.", total_files),
        schema: combined_schema,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_insert_values() {
        let rules = SanitizationRules::default();
        let columns = vec!["id".to_string(), "email".to_string(), "name".to_string()];
        let values = "(1, 'test@example.com', 'John Doe')";
        
        let (masked, modified) = mask_insert_values(values, &columns, &rules);
        assert!(modified);
        assert!(masked.contains("user_masked@example.com"));
        assert!(!masked.contains("test@example.com"));
        assert!(masked.contains("John Doe")); // Name isn't masked by default rules
    }

    #[test]
    fn test_infer_schema_from_insert_line() {
        let sql_identifier = r#"(?:`[^`]+`|"[^"]+"|\[[^\]]+\]|[A-Za-z_][A-Za-z0-9_$]*)(?:\s*\.\s*(?:`[^`]+`|"[^"]+"|\[[^\]]+\]|[A-Za-z_][A-Za-z0-9_$]*))*"#;
        let insert_regex = Regex::new(&format!(
            r#"(?is)^(?P<head>.*?\bINSERT\s+INTO\s+(?P<table>{})\s*)(?P<columns>\([^)]*\))?(?P<values_keyword>\s+VALUES\s*)(?P<values>.*)$"#,
            sql_identifier
        )).unwrap();
        
        let line = "INSERT INTO `users` (`id`, `email`) VALUES (1, 'test@test.com');";
        let (table, cols) = infer_schema_from_insert_line(line, &insert_regex).unwrap();
        assert_eq!(table, "users");
        assert_eq!(cols, vec!["id", "email"]);
    }
}
