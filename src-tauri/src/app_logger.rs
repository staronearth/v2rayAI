/// In-memory application logger that captures `log` crate messages
/// and exposes them via Tauri commands for the frontend to display.

use log::{Level, Log, Metadata, Record};
use serde::{Deserialize, Serialize};
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
    env_logger: env_logger::Logger,
}

impl AppLogger {
    pub fn init() -> &'static Self {
        let env_logger = env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .build();

        let max_level = env_logger.filter();

        let logger = Box::new(Self {
            entries: Mutex::new(Vec::with_capacity(MAX_LOGS)),
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
