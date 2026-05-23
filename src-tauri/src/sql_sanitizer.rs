use regex::Regex;
use serde::Serialize;
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

#[derive(Debug, Clone, Serialize)]
pub struct SanitizationReport {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub tables_seen: usize,
    pub insert_rows_seen: usize,
    pub replacements: usize,
}

#[derive(Debug, Clone)]
pub struct SanitizerConfig {
    pub output_path: Option<PathBuf>,
}

impl Default for SanitizerConfig {
    fn default() -> Self {
        Self { output_path: None }
    }
}

#[derive(Debug, Clone)]
struct TableShape {
    name: String,
    columns: Vec<String>,
    pii_indexes: Vec<usize>,
}

#[derive(Debug, Clone)]
enum PiiKind {
    Email,
    Password,
    Phone,
    Name,
    Address,
    Token,
    Generic,
}

#[tauri::command]
pub async fn sanitize_sql_file(
    input_path: String,
    output_path: Option<String>,
) -> Result<SanitizationReport, String> {
    sanitize_sql_dump(
        PathBuf::from(input_path),
        SanitizerConfig {
            output_path: output_path.map(PathBuf::from),
        },
    )
    .await
    .map_err(|error| error.to_string())
}

pub async fn sanitize_sql_dump<P>(
    input_path: P,
    config: SanitizerConfig,
) -> io::Result<SanitizationReport>
where
    P: AsRef<Path>,
{
    let input_path = input_path.as_ref().to_path_buf();
    let output_path = config
        .output_path
        .unwrap_or_else(|| sanitized_path(&input_path));

    let input = File::open(&input_path).await?;
    let output = File::create(&output_path).await?;
    let mut reader = BufReader::with_capacity(1024 * 1024, input);
    let mut writer = BufWriter::with_capacity(1024 * 1024, output);
    let parser = SqlSanitizer::new();
    let mut state = SanitizerState::default();
    let mut line = String::new();

    loop {
        line.clear();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            break;
        }

        let sanitized = parser.sanitize_line(&line, &mut state);
        writer.write_all(sanitized.as_bytes()).await?;
    }

    writer.flush().await?;

    Ok(SanitizationReport {
        input_path,
        output_path,
        tables_seen: state.tables_seen,
        insert_rows_seen: state.insert_rows_seen,
        replacements: state.replacements,
    })
}

fn sanitized_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("dump");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("sql");
    path.with_file_name(format!("{stem}.sanitized.{extension}"))
}

#[derive(Default)]
struct SanitizerState {
    tables: Vec<TableShape>,
    create_buffer: Option<String>,
    create_depth: i32,
    tables_seen: usize,
    insert_rows_seen: usize,
    replacements: usize,
}

struct SqlSanitizer {
    create_start: Regex,
    create_table: Regex,
    insert_into: Regex,
    pii_column: Regex,
}

impl SqlSanitizer {
    fn new() -> Self {
        Self {
            create_start: Regex::new(r"(?i)\bCREATE\s+(?:TEMPORARY\s+)?TABLE\b")
                .expect("valid CREATE TABLE start regex"),
            create_table: Regex::new(r#"(?is)\bCREATE\s+(?:TEMPORARY\s+)?TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(?P<table>(?:"[^"]+"|`[^`]+`|\[[^\]]+\]|\w+)(?:\.(?:"[^"]+"|`[^`]+`|\[[^\]]+\]|\w+))?)\s*\((?P<body>.*)\)"#)
                .expect("valid CREATE TABLE regex"),
            insert_into: Regex::new(r#"(?i)\bINSERT\s+INTO\s+(?P<table>(?:"[^"]+"|`[^`]+`|\[[^\]]+\]|\w+)(?:\.(?:"[^"]+"|`[^`]+`|\[[^\]]+\]|\w+))?)\s*(?:\((?P<columns>[^)]*)\))?\s+VALUES\s*(?P<values>.*)"#)
                .expect("valid INSERT regex"),
            pii_column: Regex::new(r#"(?i)(email|e_mail|password|passwd|pwd|phone|mobile|tel|name|first_name|last_name|full_name|address|street|city|zip|postal|token|secret|api_key|ssn)"#)
                .expect("valid PII column regex"),
        }
    }

    fn sanitize_line(&self, line: &str, state: &mut SanitizerState) -> String {
        if self.capture_create_table(line, state) {
            return line.to_owned();
        }

        if let Some((sanitized, replacements, rows)) = self.sanitize_insert(line, state) {
            state.replacements += replacements;
            state.insert_rows_seen += rows;
            return sanitized;
        }

        line.to_owned()
    }

    fn capture_create_table(&self, line: &str, state: &mut SanitizerState) -> bool {
        if let Some(buffer) = state.create_buffer.as_mut() {
            buffer.push_str(line);
            state.create_depth += paren_depth_delta(line);

            if state.create_depth <= 0 && line.contains(';') {
                if let Some(sql) = state.create_buffer.take() {
                    if let Some(table) = self.parse_create_table(&sql) {
                        state.tables_seen += 1;
                        state.tables.push(table);
                    }
                }
                state.create_depth = 0;
            }

            return true;
        }

        if !self.create_start.is_match(line) {
            return false;
        }

        state.create_buffer = Some(line.to_owned());
        state.create_depth = paren_depth_delta(line);

        if state.create_depth <= 0 && line.contains(';') {
            if let Some(sql) = state.create_buffer.take() {
                if let Some(table) = self.parse_create_table(&sql) {
                    state.tables_seen += 1;
                    state.tables.push(table);
                }
            }
            state.create_depth = 0;
        }

        true
    }

    fn parse_create_table(&self, line: &str) -> Option<TableShape> {
        let captures = self.create_table.captures(line)?;
        let name = clean_identifier(captures.name("table")?.as_str());
        let body = captures.name("body")?.as_str();
        let columns: Vec<String> = split_top_level(body)
            .into_iter()
            .filter_map(|definition| parse_column_name(&definition))
            .collect();
        let pii_indexes = columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| self.pii_column.is_match(column).then_some(index))
            .collect();

        Some(TableShape {
            name,
            columns,
            pii_indexes,
        })
    }

    fn sanitize_insert(
        &self,
        line: &str,
        state: &SanitizerState,
    ) -> Option<(String, usize, usize)> {
        let captures = self.insert_into.captures(line)?;
        let table_name = clean_identifier(captures.name("table")?.as_str());
        let shape = state
            .tables
            .iter()
            .rev()
            .find(|table| table.name.eq_ignore_ascii_case(&table_name))?;

        let insert_columns = captures
            .name("columns")
            .map(|columns| {
                split_top_level(columns.as_str())
                    .into_iter()
                    .map(|column| clean_identifier(&column))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| shape.columns.clone());

        let pii_indexes = insert_columns
            .iter()
            .enumerate()
            .filter_map(|(insert_index, column)| {
                let is_known_pii = shape
                    .columns
                    .iter()
                    .position(|known| known.eq_ignore_ascii_case(column))
                    .map(|known_index| shape.pii_indexes.contains(&known_index))
                    .unwrap_or(false);
                (is_known_pii || self.pii_column.is_match(column)).then_some(insert_index)
            })
            .collect::<Vec<_>>();

        if pii_indexes.is_empty() {
            return None;
        }

        let values_match = captures.name("values")?;
        let prefix = &line[..values_match.start()];
        let suffix = keep_line_ending(values_match.as_str());
        let mut values = values_match
            .as_str()
            .trim_end_matches(['\r', '\n'])
            .trim_end();
        let has_semicolon = values.ends_with(';');
        if has_semicolon {
            values = values.trim_end_matches(';').trim_end();
        }
        let mut replacements = 0;
        let mut rows = 0;
        let sanitized_rows = split_value_rows(values)
            .into_iter()
            .map(|row| {
                rows += 1;
                sanitize_value_row(&row, &insert_columns, &pii_indexes, &mut replacements)
            })
            .collect::<Vec<_>>();
        let statement_end = if has_semicolon { ";" } else { "" };

        Some((
            format!(
                "{prefix}{}{}{}",
                sanitized_rows.join(", "),
                statement_end,
                suffix
            ),
            replacements,
            rows,
        ))
    }
}

fn parse_column_name(definition: &str) -> Option<String> {
    let trimmed = definition.trim();
    let keyword = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if trimmed.is_empty()
        || trimmed.starts_with("--")
        || matches!(
            keyword.as_str(),
            "CONSTRAINT" | "PRIMARY" | "FOREIGN" | "UNIQUE" | "KEY" | "INDEX" | "CHECK"
        )
    {
        return None;
    }

    trimmed.split_whitespace().next().map(clean_identifier)
}

fn paren_depth_delta(line: &str) -> i32 {
    let mut depth = 0_i32;
    let mut quote = None;
    let bytes = line.as_bytes();
    let mut index = 0;

    while index < line.len() {
        let ch = bytes[index] as char;

        if let Some(active_quote) = quote {
            if ch == active_quote && !is_escaped(line, index) {
                quote = None;
            }
            index += 1;
            continue;
        }

        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }

        index += 1;
    }

    depth
}

fn sanitize_value_row(
    row: &str,
    columns: &[String],
    pii_indexes: &[usize],
    replacements: &mut usize,
) -> String {
    let trimmed = row.trim();
    let inner = trimmed
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .unwrap_or(trimmed);
    let mut values = split_top_level(inner);

    for index in pii_indexes {
        if let Some(value) = values.get_mut(*index) {
            let kind = columns
                .get(*index)
                .map(|column| pii_kind(column))
                .unwrap_or(PiiKind::Generic);
            *value = mock_value(kind, *replacements);
            *replacements += 1;
        }
    }

    format!("({})", values.join(", "))
}

fn pii_kind(column: &str) -> PiiKind {
    let lower = column.to_ascii_lowercase();
    if lower.contains("email") || lower.contains("e_mail") {
        PiiKind::Email
    } else if lower.contains("password") || lower.contains("passwd") || lower == "pwd" {
        PiiKind::Password
    } else if lower.contains("phone") || lower.contains("mobile") || lower.contains("tel") {
        PiiKind::Phone
    } else if lower.contains("name") {
        PiiKind::Name
    } else if lower.contains("address")
        || lower.contains("street")
        || lower.contains("city")
        || lower.contains("zip")
        || lower.contains("postal")
    {
        PiiKind::Address
    } else if lower.contains("token") || lower.contains("secret") || lower.contains("key") {
        PiiKind::Token
    } else {
        PiiKind::Generic
    }
}

fn mock_value(kind: PiiKind, index: usize) -> String {
    match kind {
        PiiKind::Email => format!("'user{}@example.test'", index + 1),
        PiiKind::Password => "'$argon2id$redacted-password-hash'".to_owned(),
        PiiKind::Phone => format!("'+155501{:04}'", index % 10_000),
        PiiKind::Name => format!("'Sample User {}'", index + 1),
        PiiKind::Address => format!("'{} Example Street'", index + 100),
        PiiKind::Token => format!("'mock_token_{:08}'", index + 1),
        PiiKind::Generic => "'REDACTED'".to_owned(),
    }
}

fn split_value_rows(values: &str) -> Vec<String> {
    let mut rows = split_top_level(values);
    if rows.len() == 1 && !rows[0].trim_start().starts_with('(') {
        rows = vec![values.to_owned()];
    }
    rows
}

fn split_top_level(input: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut quote = None;
    let bytes = input.as_bytes();
    let mut index = 0;

    while index < input.len() {
        let ch = bytes[index] as char;

        if let Some(active_quote) = quote {
            if ch == active_quote && !is_escaped(input, index) {
                quote = None;
            }
            index += 1;
            continue;
        }

        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(input[start..index].trim().to_owned());
                start = index + 1;
            }
            _ => {}
        }

        index += 1;
    }

    let last = input[start..].trim();
    if !last.is_empty() {
        parts.push(last.to_owned());
    }
    parts
}

fn is_escaped(input: &str, index: usize) -> bool {
    index > 0 && input.as_bytes()[index - 1] == b'\\'
}

fn clean_identifier(identifier: &str) -> String {
    identifier
        .trim()
        .trim_matches(',')
        .split('.')
        .map(|part| {
            part.trim()
                .trim_matches('"')
                .trim_matches('`')
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn keep_line_ending(value: &str) -> &'static str {
    if value.ends_with("\r\n") {
        "\r\n"
    } else if value.ends_with('\n') {
        "\n"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_insert_values_for_pii_columns() {
        let parser = SqlSanitizer::new();
        let mut state = SanitizerState::default();

        parser.sanitize_line(
            "CREATE TABLE users (id int, email varchar(255), password text, phone text);\n",
            &mut state,
        );
        let out = parser.sanitize_line(
            "INSERT INTO users VALUES (1, 'real@example.com', 'secret', '123');\n",
            &mut state,
        );

        assert!(out.contains("'user1@example.test'"));
        assert!(out.contains("'$argon2id$redacted-password-hash'"));
        assert!(out.contains("'+1555010002'"));
        assert_eq!(state.replacements, 3);
    }

    #[test]
    fn respects_insert_column_order() {
        let parser = SqlSanitizer::new();
        let mut state = SanitizerState::default();

        parser.sanitize_line(
            "CREATE TABLE users (id int, email text, display_name text);\n",
            &mut state,
        );
        let out = parser.sanitize_line(
            "INSERT INTO users (display_name, id, email) VALUES ('Ada', 7, 'ada@example.com');",
            &mut state,
        );

        assert!(out.contains("'Sample User 1', 7, 'user2@example.test'"));
    }
}
