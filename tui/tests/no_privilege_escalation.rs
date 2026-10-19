use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN: [&str; 5] = ["sudo", "pkexec", "doas", "su", "run0"];

fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).filter(|word| !word.is_empty())
}

fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = vec![root.join("export-themes")];
    for dir in [root.join("src"), root.join("../bin")] {
        for entry in fs::read_dir(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display())) {
            files.push(entry.unwrap().path());
        }
    }
    files
}

#[test]
fn never_escalates_privileges() {
    let files = sources();
    assert!(files.iter().any(|file| file.ends_with("rumb-scan")), "rumb-scan not found");
    for file in files {
        let text = fs::read_to_string(&file).unwrap_or_else(|error| panic!("{}: {error}", file.display()));
        for (number, line) in text.lines().enumerate() {
            let place = format!("{}:{}", file.display(), number + 1);
            if let Some(word) = words(line).find(|word| FORBIDDEN.contains(word)) {
                panic!("{place} names {word}: {line}");
            }
            if words(line).any(|word| word == "checkupdates") {
                assert!(!words(line).any(|word| word == "-d" || word == "--download"), "{place} lets checkupdates download as root: {line}");
            }
        }
    }
}
