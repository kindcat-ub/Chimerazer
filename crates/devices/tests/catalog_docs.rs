//! Keeps device command catalogs, `docs/protocol/*.md` tables and
//! `docs/protocol/SOURCES.md` in sync.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chimerazer_devices::{CommandSpec, naga_pro};

struct Row {
    size: String,
    status: String,
    sources: Vec<String>,
}

fn docs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/protocol")
}

fn read(name: &str) -> String {
    let path = docs().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn cell(s: &str) -> String {
    s.trim().trim_matches('`').to_string()
}

/// Command table rows keyed by `CC:II/SS` (command and size): one command
/// may have several argument layouts. A row starts with a backticked command.
fn command_rows(markdown: &str) -> BTreeMap<String, Row> {
    let mut rows = BTreeMap::new();
    for line in markdown.lines().filter(|l| l.starts_with("| `")) {
        let cols: Vec<&str> = line.trim_matches('|').split('|').collect();
        let command = cell(cols[0]);
        if command.len() != 5 || command.as_bytes()[2] != b':' {
            continue;
        }
        let size = cell(cols[2]);
        let key = format!("{command}/{size}");
        let row = Row {
            size,
            status: cell(cols[4]),
            sources: cols[5]
                .split(',')
                .map(cell)
                .filter(|s| !s.is_empty())
                .collect(),
        };
        assert!(
            rows.insert(key.clone(), row).is_none(),
            "{key} documented twice"
        );
    }
    rows
}

fn is_read(command: &str) -> bool {
    u8::from_str_radix(&command[3..5], 16).unwrap() & 0x80 != 0
}

fn defined_sources() -> Vec<String> {
    read("SOURCES.md")
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| cell(l.trim_matches('|').split('|').next().unwrap()))
        .collect()
}

fn check(doc: &str, catalog: &[CommandSpec]) {
    let rows = command_rows(&read(doc));
    let sources = defined_sources();

    for spec in catalog {
        let key = format!("{}/{:02X}", spec.command, spec.command.size);
        let row = rows
            .get(&key)
            .unwrap_or_else(|| panic!("{key} missing from {doc}"));
        assert_eq!(
            row.size,
            format!("{:02X}", spec.command.size),
            "{key}: size"
        );
        assert_eq!(row.status, spec.confirmation.label(), "{key}: status");
        let code: Vec<&str> = spec.sources.to_vec();
        assert_eq!(row.sources, code, "{key}: sources");
    }

    for (key, row) in &rows {
        if is_read(key) {
            assert!(
                catalog
                    .iter()
                    .any(|s| format!("{}/{:02X}", s.command, s.command.size) == *key),
                "{doc} documents read {key} that the code does not have"
            );
        }
        assert!(!row.sources.is_empty(), "{key}: no source");
        for id in &row.sources {
            assert!(sources.contains(id), "{key}: source {id} not in SOURCES.md");
        }
        assert!(
            ["OurHardware", "ExternalHardware", "SourceOnly"].contains(&row.status.as_str()),
            "{key}: unknown status {}",
            row.status
        );
    }
}

#[test]
fn naga_pro_catalog_matches_docs() {
    let all: Vec<CommandSpec> = naga_pro::COMMANDS
        .iter()
        .chain(naga_pro::WRITE_COMMANDS)
        .copied()
        .collect();
    check("naga-pro.md", &all);
}
