/// In-memory application logger that captures `log` crate messages
/// and exposes them via Tauri commands for the frontend to display.

use log::{Level, Log, Metadata, Record};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: i64,
    pub level: String,
    pub target: String,
    pub message: String,
}

const MAX_LOGS: usize = 1000;

pub struct AppLogger {
    entries: Mutex<Vec<LogEntry>>,
    log_path: PathBuf,
    env_logger: env_logger::Logger,
}

impl AppLogger {
    pub fn init() -> &'static Self {
        let env_logger = env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .build();

        let max_level = env_logger.filter();

        let log_path = app_log_path();
        let entries = load_recent_entries(&log_path, MAX_LOGS);

        let logger = Box::new(Self {
            entries: Mutex::new(entries),
            log_path,
            env_logger,
        });

        let logger_ref: &'static Self = Box::leak(logger);
        log::set_logger(logger_ref).ok();
        log::set_max_level(max_level);
        logger_ref
    }

    pub fn get_logs(&self, count: usize, level_filter: Option<&str>) -> Vec<LogEntry> {
        let entries = self.entries.lock().unwrap();
        let iter = entries.iter().rev();

        let filtered: Vec<LogEntry> = if let Some(filter) = level_filter {
            let filter_upper = filter.to_uppercase();
            iter.filter(|e| e.level == filter_upper)
                .take(count)
                .cloned()
                .collect()
        } else {
            iter.take(count).cloned().collect()
        };

        filtered.into_iter().rev().collect()
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
        let _ = std::fs::remove_file(&self.log_path);
    }

    pub fn add_entry(&self, level: &str, target: &str, message: &str) {
        let entry = LogEntry {
            timestamp: chrono::Utc::now().timestamp_millis(),
            level: level.to_uppercase(),
            target: target.to_string(),
            message: message.to_string(),
        };

        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= MAX_LOGS {
            entries.drain(0..200); // Remove oldest 200 when full
        }
        entries.push(entry);
        if let Some(entry) = entries.last() {
            append_entry(&self.log_path, entry);
        }
    }
}

fn app_log_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("logs")
        .join("app.log")
}

fn load_recent_entries(path: &PathBuf, count: usize) -> Vec<LogEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::with_capacity(MAX_LOGS);
    };

    let mut entries: Vec<LogEntry> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<LogEntry>(line).ok())
        .collect();

    if entries.len() > count {
        entries.drain(0..entries.len() - count);
    }
    entries
}

fn append_entry(path: &PathBuf, entry: &LogEntry) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(line) = serde_json::to_string(entry) else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{}", line);
    }
}

impl Log for AppLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.env_logger.enabled(metadata) || metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        // Forward to env_logger for terminal output
        if self.env_logger.enabled(record.metadata()) {
            self.env_logger.log(record);
        }

        // Capture into our ring buffer
        if record.level() <= Level::Info {
            self.add_entry(
                &record.level().to_string(),
                record.target(),
                &format!("{}", record.args()),
            );
        }
    }

    fn flush(&self) {
        self.env_logger.flush();
    }
}
