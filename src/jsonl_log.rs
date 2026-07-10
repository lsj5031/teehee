//! JSONL structured logging for teehee.
//!
//! One JSON object per line, UTF-8, suitable for Task Scheduler redirect
//! or `--log-file` so operators can `jq` / ship logs without scraping
//! tracing text. No serde dependency — fields are hand-serialised.
//!
//! Example line:
//! ```text
//! {"ts_ms":1710000000123,"event":"send_stats","packets_sent":1200,"capture_ring_ms":12}
//! ```

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single JSON value for structured fields.
#[derive(Debug, Clone)]
pub enum JsonVal {
    Null,
    Bool(bool),
    U64(u64),
    I64(i64),
    F64(f64),
    Str(String),
}

impl From<bool> for JsonVal {
    fn from(v: bool) -> Self {
        JsonVal::Bool(v)
    }
}
impl From<u64> for JsonVal {
    fn from(v: u64) -> Self {
        JsonVal::U64(v)
    }
}
impl From<u32> for JsonVal {
    fn from(v: u32) -> Self {
        JsonVal::U64(v as u64)
    }
}
impl From<u8> for JsonVal {
    fn from(v: u8) -> Self {
        JsonVal::U64(v as u64)
    }
}
impl From<u16> for JsonVal {
    fn from(v: u16) -> Self {
        JsonVal::U64(v as u64)
    }
}
impl From<usize> for JsonVal {
    fn from(v: usize) -> Self {
        JsonVal::U64(v as u64)
    }
}
impl From<i64> for JsonVal {
    fn from(v: i64) -> Self {
        JsonVal::I64(v)
    }
}
impl From<f64> for JsonVal {
    fn from(v: f64) -> Self {
        JsonVal::F64(v)
    }
}
impl From<String> for JsonVal {
    fn from(v: String) -> Self {
        JsonVal::Str(v)
    }
}
impl From<&str> for JsonVal {
    fn from(v: &str) -> Self {
        JsonVal::Str(v.to_string())
    }
}

/// Thread-safe append-only JSONL writer. Cheap no-op when no path is set.
pub struct JsonlLogger {
    path: Option<PathBuf>,
    file: Mutex<Option<std::fs::File>>,
}

impl JsonlLogger {
    /// Open (or create) `path` for append. `None` disables all writes.
    pub fn open(path: Option<&Path>) -> io::Result<Self> {
        match path {
            None => Ok(Self {
                path: None,
                file: Mutex::new(None),
            }),
            Some(p) => {
                if let Some(parent) = p.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                let f = OpenOptions::new().create(true).append(true).open(p)?;
                Ok(Self {
                    path: Some(p.to_path_buf()),
                    file: Mutex::new(Some(f)),
                })
            }
        }
    }

    /// Whether this logger will write anything.
    pub fn enabled(&self) -> bool {
        self.path.is_some()
    }

    /// Path being written, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Emit one JSON object line: `{"ts_ms":…,"event":"<event>",…fields}`.
    pub fn emit(&self, event: &str, fields: &[(&str, JsonVal)]) {
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let Some(file) = guard.as_mut() else {
            return;
        };
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let mut line = String::with_capacity(256);
        line.push('{');
        write_kv(&mut line, "ts_ms", &JsonVal::U64(ts_ms));
        line.push(',');
        write_kv(&mut line, "event", &JsonVal::Str(event.to_string()));
        for (k, v) in fields {
            line.push(',');
            write_kv(&mut line, k, v);
        }
        line.push('}');
        line.push('\n');
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

fn write_kv(out: &mut String, key: &str, val: &JsonVal) {
    out.push('"');
    escape_str(out, key);
    out.push('"');
    out.push(':');
    write_val(out, val);
}

fn write_val(out: &mut String, val: &JsonVal) {
    match val {
        JsonVal::Null => out.push_str("null"),
        JsonVal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JsonVal::U64(n) => out.push_str(&n.to_string()),
        JsonVal::I64(n) => out.push_str(&n.to_string()),
        JsonVal::F64(f) => {
            if f.is_finite() {
                // Compact but always valid JSON number (no NaN/Inf).
                let s = format!("{f:.6}");
                // Trim trailing zeros after decimal for readability.
                let s = s.trim_end_matches('0').trim_end_matches('.');
                if s.is_empty() || s == "-" {
                    out.push('0');
                } else {
                    out.push_str(s);
                }
            } else {
                out.push_str("null");
            }
        }
        JsonVal::Str(s) => {
            out.push('"');
            escape_str(out, s);
            out.push('"');
        }
    }
}

fn escape_str(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use std::io::Read;

    #[test]
    fn disabled_logger_is_noop() {
        let log = JsonlLogger::open(None).unwrap();
        assert!(!log.enabled());
        log.emit("test", &[("x", JsonVal::U64(1))]);
    }

    #[test]
    fn writes_one_json_object_per_line() {
        let dir = std::env::temp_dir().join(format!("teehee_jsonl_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.jsonl");
        let log = JsonlLogger::open(Some(&path)).unwrap();
        log.emit(
            "send_stats",
            &[
                ("packets_sent", JsonVal::U64(42)),
                ("capture_ring_ms", JsonVal::U64(12)),
                ("rate", JsonVal::F64(50.5)),
                ("ok", JsonVal::Bool(true)),
                ("note", JsonVal::Str("hi\"there".into())),
            ],
        );
        let mut s = String::new();
        std::fs::File::open(&path)
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        let line = s.lines().next().expect("one line");
        assert!(line.starts_with('{') && line.ends_with('}'));
        assert!(line.contains("\"event\":\"send_stats\""));
        assert!(line.contains("\"packets_sent\":42"));
        assert!(line.contains("\"capture_ring_ms\":12"));
        assert!(line.contains("\"ok\":true"));
        assert!(line.contains("\"note\":\"hi\\\"there\""));
        assert!(line.contains("\"ts_ms\":"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
