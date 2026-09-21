//! P0 redaction: innate (generic patterns) + adaptive (slot-aware) layers.
//!
//! Design (approved 2026-09-12): static Experience templates are "self" and
//! are not redacted; runtime bindings are classified by parameter slot;
//! secrets are always masked; paths keep their workspace-relative form;
//! payloads default to hash+length; identity-like values use a per-store
//! HMAC ("comparable but irreversible"). Markers are idempotent.

use serde_json::Map;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disclosure {
    MetadataOnly,
    Structure,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactMode {
    Off,
    Standard,
    Strict,
}

#[derive(Clone)]
pub struct Redactor {
    key: Vec<u8>,
    disclosure: Disclosure,
    mode: RedactMode,
}

const SECRET_KEYS: &[&str] = &[
    "key", "token", "secret", "password", "passwd", "credential", "authorization", "auth",
    "apikey",
];
const PAYLOAD_KEYS: &[&str] = &["content", "stdin", "cmd", "command", "script", "code", "body"];
const URL_KEYS: &[&str] = &["url", "endpoint", "base_url", "uri"];

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| haystack.contains(needle) && !haystack.contains("<masked:"))
}

fn is_absolute_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic())
        || value.starts_with(r"\\")
        || value.starts_with('/')
        || value.starts_with('\\')
}

fn looks_id_like(value: &str) -> bool {
    let compact: String = value.chars().filter(|ch| ch.is_ascii_alphanumeric()).collect();
    compact.len() >= 16
        && compact
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch == '-')
}

impl Redactor {
    pub fn new(key: &[u8], disclosure: Disclosure, mode: RedactMode) -> Self {
        Self {
            key: key.to_vec(),
            disclosure,
            mode,
        }
    }

    pub fn disclosure(&self) -> Disclosure {
        self.disclosure
    }

    pub fn mode(&self) -> RedactMode {
        self.mode
    }

    fn hmac_token(&self, kind: &str, value: &str) -> String {
        let digest = hmac_sha256(&self.key, value.as_bytes());
        format!("<{kind}:hmac:{}>", hex_prefix(&digest, 8))
    }

    fn payload_token(&self, value: &str) -> String {
        if self.disclosure == Disclosure::Content {
            return value.to_string();
        }
        let digest = sha256(value.as_bytes());
        let len = value.chars().count();
        match self.disclosure {
            Disclosure::MetadataOnly => {
                format!("<payload:hmac:{} len={len}>", hex_prefix(&hmac_sha256(&self.key, value.as_bytes()), 8))
            }
            _ => format!("<payload:sha256:{} len={len}>", hex_prefix(&digest, 8)),
        }
    }

    fn path_value(&self, value: &str) -> String {
        if is_absolute_like(value) {
            return "<path:external>".to_string();
        }
        match self.disclosure {
            Disclosure::MetadataOnly => self.hmac_token("path", value),
            _ => value.to_string(),
        }
    }

    fn url_value(&self, value: &str) -> String {
        if self.disclosure == Disclosure::Content {
            return value.to_string();
        }
        let masked_userinfo = if let Some((scheme, rest)) = value.split_once("://") {
            if let Some((userinfo, host_and_path)) = rest.split_once('@') {
                if userinfo.contains(':') {
                    format!("{scheme}://<masked:userinfo>@{host_and_path}")
                } else {
                    value.to_string()
                }
            } else {
                value.to_string()
            }
        } else {
            value.to_string()
        };
        let (scheme, rest) = match masked_userinfo.split_once("://") {
            Some(parts) => parts,
            None => return masked_userinfo,
        };
        let (host, path) = match rest.split_once('/') {
            Some((host, path)) => (host, format!("/{path}")),
            None => (rest, String::new()),
        };
        if host.starts_with("<masked") {
            return masked_userinfo;
        }
        format!("{scheme}://{}{}", self.hmac_token("host", host), path)
    }

    fn id_value(&self, value: &str) -> String {
        if self.disclosure == Disclosure::Content {
            return value.to_string();
        }
        match self.disclosure {
            Disclosure::MetadataOnly => "<id>".to_string(),
            _ => self.hmac_token("id", value),
        }
    }

    /// Adaptive layer: classify a JSON value by its field key.
    pub fn redact_json(&self, value: &Value, key_hint: Option<&str>) -> Value {
        if self.mode == RedactMode::Off {
            return value.clone();
        }
        match value {
            Value::Object(map) => {
                let mut out = Map::new();
                for (key, item) in map {
                    out.insert(key.clone(), self.redact_json(item, Some(key)));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.redact_json(item, key_hint))
                    .collect(),
            ),
            Value::String(text) => {
                if is_redaction_marker(text) {
                    return value.clone();
                }
                let key = key_hint.unwrap_or("").to_lowercase();
                if contains_any(&key, SECRET_KEYS) {
                    return Value::String("<masked:secret>".to_string());
                }
                if contains_any(&key, PAYLOAD_KEYS) {
                    return Value::String(self.payload_token(text));
                }
                if is_absolute_like(text) {
                    return Value::String(self.path_value(text));
                }
                if contains_any(&key, URL_KEYS) || text.starts_with("http://") || text.starts_with("https://") {
                    return Value::String(self.url_value(text));
                }
                if looks_id_like(text) {
                    return Value::String(self.id_value(text));
                }
                Value::String(self.redact_text(text))
            }
            other => other.clone(),
        }
    }

    /// Innate layer: small, bounded pattern set; idempotent.
    pub fn redact_text(&self, text: &str) -> String {
        if self.mode == RedactMode::Off {
            return text.to_string();
        }
        let mut out_lines: Vec<String> = Vec::new();
        let mut in_private_key = false;
        for line in text.lines() {
            if line.contains("PRIVATE KEY-----") {
                in_private_key = !line.contains("END PRIVATE KEY");
                out_lines.push("<masked:private_key>".to_string());
                continue;
            }
            if in_private_key {
                if line.contains("END PRIVATE KEY") {
                    in_private_key = false;
                }
                continue;
            }
            out_lines.push(self.redact_line(line));
        }
        let mut result = out_lines.join("\n");
        if text.ends_with('\n') {
            result.push('\n');
        }
        result
    }

    fn redact_line(&self, line: &str) -> String {
        for marker in ["<masked:", "<payload:", "<id:", "<path:", "<host:"] {
            if line.contains(marker) {
                return line.to_string();
            }
        }
        let mut result = line.to_string();
        // key = value / key: value
        let lower = result.to_lowercase();
        let separator = lower
            .char_indices()
            .find(|(_, ch)| *ch == ':' || *ch == '=')
            .map(|(index, _)| index);
        if let Some(index) = separator {
            let left = &lower[..index];
            if contains_any(left, SECRET_KEYS) {
                // Secret-bearing line: mask the whole value part.
                result = format!("{} <masked:secret>", result[..index].trim_end());
            }
        }
        result = replace_case_insensitive(&result, "bearer ", "<masked:bearer>", self.mode);
        result = mask_url_userinfo(&result);
        result = mask_prefixed_tokens(&result, self.mode);
        result
    }
}

/// Keyless innate redaction for producers that have no store key yet
/// (e.g. agent-codex args extraction). Identity-like values become `<id>`.
pub fn innate_redact(text: &str) -> String {
    let redactor = Redactor::new(b"innate", Disclosure::Structure, RedactMode::Standard);
    redactor.redact_text(text)
}

fn is_redaction_marker(text: &str) -> bool {
    text == "<id>"
        || text.starts_with("<masked:")
        || text.starts_with("<payload:")
        || text.starts_with("<id:")
        || text.starts_with("<path:")
        || text.starts_with("<host:")
}

fn replace_case_insensitive(text: &str, needle: &str, marker: &str, mode: RedactMode) -> String {
    if mode == RedactMode::Off {
        return text.to_string();
    }
    let lower = text.to_lowercase();
    let mut start = 0;
    let mut out = String::new();
    while let Some(found) = lower[start..].find(needle) {
        let absolute = start + found;
        out.push_str(&text[start..absolute]);
        out.push_str(marker);
        let after = absolute + needle.len();
        let tail = &text[after..];
        let token_end = tail
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace() || *ch == ',' || *ch == ';' || *ch == '"' || *ch == '\'')
            .map(|(index, _)| index)
            .unwrap_or(0);
        if token_end == 0 {
            start = after;
        } else {
            start = after + token_end;
        }
    }
    out.push_str(&text[start..]);
    out
}

fn mask_url_userinfo(text: &str) -> String {
    let Some(scheme_at) = text.find("://") else {
        return text.to_string();
    };
    let rest = &text[scheme_at + 3..];
    let Some(at) = rest.find('@') else {
        return text.to_string();
    };
    let userinfo = &rest[..at];
    if !userinfo.contains(':') || userinfo.contains("<masked:") {
        return text.to_string();
    }
    format!("{}://<masked:userinfo>@{}", &text[..scheme_at], &rest[at + 1..])
}

fn mask_prefixed_tokens(text: &str, mode: RedactMode) -> String {
    if mode == RedactMode::Off {
        return text.to_string();
    }
    let mut out = String::new();
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        let lower = token.to_lowercase();
        let prefixed = lower.starts_with("sk-")
            || lower.starts_with("ghp_")
            || lower.starts_with("gho_")
            || lower.starts_with("akia")
            || lower.starts_with("xoxb-");
        let opaque = mode == RedactMode::Strict
            && token.len() >= 24
            && token.chars().any(|ch| ch.is_ascii_digit())
            && token.chars().any(|ch| ch.is_ascii_alphabetic());
        if prefixed || opaque {
            out.push_str("<masked:key>");
        } else {
            out.push_str(token);
        }
        token.clear();
    };
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            token.push(ch);
        } else {
            flush(&mut token, &mut out);
            out.push(ch);
        }
    }
    flush(&mut token, &mut out);
    out
}

// --- SHA-256 / HMAC-SHA256 (self-contained, no external deps) ---

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for index in 0..16 {
            w[index] = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (index, value) in h.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    out
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut normalized = if key.len() > 64 { sha256(key).to_vec() } else { key.to_vec() };
    normalized.resize(64, 0);
    let mut inner = Vec::with_capacity(64 + data.len());
    let mut outer = Vec::with_capacity(64 + 32);
    for byte in &normalized {
        inner.push(byte ^ 0x36);
        outer.push(byte ^ 0x5c);
    }
    inner.extend_from_slice(data);
    let inner_hash = sha256(&inner);
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

fn hex_prefix(bytes: &[u8], count: usize) -> String {
    let mut out = String::new();
    for byte in bytes.iter().take(count) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn redactor(disclosure: Disclosure, mode: RedactMode) -> Redactor {
        Redactor::new(b"test-key", disclosure, mode)
    }

    #[test]
    fn sha256_and_hmac_match_known_vectors() {
        let digest = sha256(b"abc");
        assert_eq!(
            hex_prefix(&digest, 32),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mac = hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            hex_prefix(&mac, 32),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn innate_patterns_mask_secrets_and_are_idempotent() {
        let text = "API_KEY=sk-abc123XYZ\nAuthorization: Bearer abc.def.ghi\nurl=https://user:pass@example.com/a";
        let redactor = redactor(Disclosure::Structure, RedactMode::Standard);
        let once = redactor.redact_text(text);
        assert!(!once.contains("sk-abc123XYZ"));
        assert!(!once.contains("abc.def.ghi"));
        assert!(!once.contains("user:pass"));
        assert!(once.contains("<masked:"));
        let twice = redactor.redact_text(&once);
        assert_eq!(once, twice, "redaction must be idempotent");
    }

    #[test]
    fn slot_policy_keeps_relative_paths_and_hashes_payloads() {
        let redactor = redactor(Disclosure::Structure, RedactMode::Standard);
        let value = json!({
            "path": "src/index.ts",
            "absolute": r"C:\secret\x.txt",
            "content": "hello world",
            "token": "super-secret",
            "url": "https://user:pass@example.com/api",
            "id": "550e8400-e29b-41d4-a716-446655440000"
        });
        let redacted = redactor.redact_json(&value, None);
        assert_eq!(redacted["path"], "src/index.ts");
        assert_eq!(redacted["absolute"], "<path:external>");
        assert!(redacted["content"].as_str().unwrap().starts_with("<payload:sha256:"));
        assert_eq!(redacted["token"], "<masked:secret>");
        assert!(redacted["url"].as_str().unwrap().contains("<masked:userinfo>"));
        assert!(redacted["id"].as_str().unwrap().starts_with("<id:hmac:"));
        let text = serde_json::to_string(&redacted).unwrap();
        assert!(!text.contains("super-secret"));
        assert!(!text.contains("hello world"));
    }

    #[test]
    fn content_disclosure_keeps_payloads_but_never_secrets() {
        let redactor = redactor(Disclosure::Content, RedactMode::Standard);
        let value = json!({"content": "keep me", "password": "hide me"});
        let redacted = redactor.redact_json(&value, None);
        assert_eq!(redacted["content"], "keep me");
        assert_eq!(redacted["password"], "<masked:secret>");
    }

    #[test]
    fn hmac_is_comparable_but_irreversible() {
        let redactor = redactor(Disclosure::Structure, RedactMode::Standard);
        let first = redactor.redact_json(&json!({"id": "abc1234567890abc"}), None);
        let second = redactor.redact_json(&json!({"id": "abc1234567890abc"}), None);
        let other = redactor.redact_json(&json!({"id": "different-value-1234"}), None);
        assert_eq!(first, second);
        assert_ne!(first, other);
        assert!(!first["id"].as_str().unwrap().contains("abc1234567890abc"));
    }
}
