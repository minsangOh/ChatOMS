use std::{fmt, ops::Deref};

use chatoms_ports::error::{CategorizedFailure, FailureCategory};
use regex::Regex;
use thiserror::Error;

pub const MAX_REDACTION_INPUT_BYTES: usize = 65_536;
pub const REDACTED_MARKER: &str = "[REDACTED]";
pub const REDACTION_FAILED_MARKER: &str = "[REDACTION_FAILED]";
pub const TRUNCATED_MARKER: &str = "[TRUNCATED]";

#[derive(Clone, Eq, PartialEq)]
pub struct RedactedText(String);

impl RedactedText {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for RedactedText {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl fmt::Display for RedactedText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for RedactedText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RedactedText")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactionReport {
    pub text: RedactedText,
    pub replacement_count: usize,
    pub truncated: bool,
    pub failed_closed: bool,
}

#[derive(Debug, Error)]
pub enum RedactionError {
    #[error("redaction rules could not be initialized")]
    RuleInitialization(#[source] regex::Error),
    #[error("redacted output did not pass sensitive-material validation")]
    UnsafeOutput,
}

impl CategorizedFailure for RedactionError {
    fn category(&self) -> FailureCategory {
        FailureCategory::RedactionFailure
    }
}

struct Rule {
    regex: Regex,
    marker: &'static str,
}

pub struct SecretRedactor {
    rules: Vec<Rule>,
    sensitive_field_name: Regex,
}

impl SecretRedactor {
    pub fn new() -> Result<Self, RedactionError> {
        let specifications = [
            (
                r"(?is)-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----.*?(?:-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----|\z)",
                "[REDACTED:PRIVATE_KEY]",
            ),
            (
                r"(?i)(?:proxy-)?authorization\s*:\s*(?:bearer|basic)[\t \r\n]+[^\s,;]+",
                "[REDACTED:AUTHORIZATION]",
            ),
            (
                r"(?im)^(?:cookie|set-cookie)\s*:[^\r\n]*",
                "[REDACTED:COOKIE]",
            ),
            (
                r#"(?i)\b(?:api[_-]?key|token|access_token|refresh_token|id_token|client_secret|secret|password|passwd|session|session_id|csrf(?:_token)?)\b["']?\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,;&}\]]+)"#,
                REDACTED_MARKER,
            ),
            (
                r#"(?i)--(?:api-key|token|access-token|refresh-token|client-secret|password|session)\s+(?:"[^"]*"|'[^']*'|[^\s]+)"#,
                REDACTED_MARKER,
            ),
            (
                r"(?i)\b(?:ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{16,})\b",
                "[REDACTED:PROVIDER_TOKEN]",
            ),
            (
                r"\b[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b",
                "[REDACTED:JWT]",
            ),
            (
                r"(?i)https?://[^\s/:@]+:[^\s/@]+@",
                "[REDACTED:URL_CREDENTIAL]@",
            ),
        ];
        let mut rules = Vec::with_capacity(specifications.len());
        for (pattern, marker) in specifications {
            rules.push(Rule {
                regex: Regex::new(pattern).map_err(RedactionError::RuleInitialization)?,
                marker,
            });
        }
        let sensitive_field_name = Regex::new(
            r"(?i)^(?:authorization|proxy_authorization|cookie|set_cookie|api[_-]?key|token|access_token|refresh_token|id_token|client_secret|secret|password|passwd|session|session_id|csrf(?:_token)?)$",
        )
        .map_err(RedactionError::RuleInitialization)?;
        Ok(Self {
            rules,
            sensitive_field_name,
        })
    }

    pub fn redact_text(&self, input: &str) -> RedactionReport {
        let (bounded, truncated) = bounded_prefix(input);
        let mut output = bounded.to_owned();
        let mut replacement_count = 0;
        for rule in &self.rules {
            replacement_count += rule.regex.find_iter(&output).count();
            output = rule.regex.replace_all(&output, rule.marker).into_owned();
        }

        let decoded_sensitive = decoded_variants(bounded)
            .into_iter()
            .any(|variant| variant != bounded && self.contains_sensitive_direct(&variant));
        let failed_closed = decoded_sensitive && replacement_count == 0;
        if failed_closed || self.contains_sensitive_direct(&output) {
            output.clear();
            output.push_str(REDACTION_FAILED_MARKER);
            replacement_count = replacement_count.saturating_add(1);
        }
        if truncated {
            output.push_str(TRUNCATED_MARKER);
        }
        RedactionReport {
            text: RedactedText(output),
            replacement_count,
            truncated,
            failed_closed,
        }
    }

    pub fn redact_field(&self, name: &str, value: &str) -> RedactionReport {
        if self.sensitive_field_name.is_match(name.trim()) {
            RedactionReport {
                text: RedactedText(REDACTED_MARKER.to_owned()),
                replacement_count: 1,
                truncated: value.len() > MAX_REDACTION_INPUT_BYTES,
                failed_closed: false,
            }
        } else {
            self.redact_text(value)
        }
    }

    #[must_use]
    pub fn contains_sensitive_material(&self, input: &str) -> bool {
        if input.len() > MAX_REDACTION_INPUT_BYTES || self.contains_sensitive_direct(input) {
            return true;
        }
        decoded_variants(input)
            .into_iter()
            .any(|variant| variant != input && self.contains_sensitive_direct(&variant))
    }

    pub fn validate_redacted(&self, output: &str) -> Result<RedactedText, RedactionError> {
        if self.contains_sensitive_material(output) {
            Err(RedactionError::UnsafeOutput)
        } else {
            Ok(RedactedText(output.to_owned()))
        }
    }

    fn contains_sensitive_direct(&self, input: &str) -> bool {
        self.rules.iter().any(|rule| rule.regex.is_match(input))
    }
}

fn bounded_prefix(input: &str) -> (&str, bool) {
    if input.len() <= MAX_REDACTION_INPUT_BYTES {
        return (input, false);
    }
    let mut end = MAX_REDACTION_INPUT_BYTES;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    (&input[..end], true)
}

fn decoded_variants(input: &str) -> Vec<String> {
    let mut variants = Vec::with_capacity(2);
    if let Some(decoded) = percent_decode_once(input)
        && decoded != input
    {
        variants.push(decoded);
    }
    if let Some(decoded) = json_unescape_once(input)
        && decoded != input
    {
        variants.push(decoded);
    }
    variants
}

fn percent_decode_once(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let mut changed = false;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            output.push(high * 16 + low);
            index += 3;
            changed = true;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    changed.then(|| String::from_utf8(output).ok()).flatten()
}

fn json_unescape_once(input: &str) -> Option<String> {
    let mut output = String::with_capacity(input.len());
    let mut characters = input.chars();
    let mut changed = false;
    while let Some(character) = characters.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        let escaped = characters.next()?;
        let decoded = match escaped {
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            _ => return None,
        };
        output.push(decoded);
        changed = true;
    }
    changed.then_some(output)
}

const fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
